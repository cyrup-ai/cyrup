use super::*;

impl<B: Backend> App<B> {
    /// Build an app over `backend` using a **content-sized inline viewport** (R-ARCH-TUI-003,
    /// ADR-0001 #1): the live region holds only the active turn + status band + editor/selector +
    /// footer, so finished history flushes to native scrollback (`insert_before`) instead of the
    /// inline region swallowing the whole screen. No alternate screen is entered.
    pub fn new(backend: B, theme: UiTheme) -> Result<Self, TuiError> {
        let size = backend.size().map_err(|e| TuiError::Backend(e.to_string()))?;
        let mut state = AppState::new(theme);
        let height = live_region_height(&mut state, size.width, size.height.max(1));
        let terminal = Terminal::with_options(
            backend,
            TerminalOptions { viewport: Viewport::Inline(height.max(1)) },
        )
        .map_err(|e| TuiError::Backend(e.to_string()))?;
        // Seed `0` so the first `draw` always rebuilds the viewport bottom-anchored (the constructed
        // `Terminal` is top-anchored at the backend's initial cursor; the rebuild fixes the anchor).
        Ok(App {
            terminal,
            state,
            viewport_height: 0,
            live_floor: 0,
            tree_nav_tx: None,
            package_update_rx: None,
            login_tx: None,
            login_providers: None,
            compact_tx: None,
            queue_drain_tx: None,
            lifecycle_tx: None,
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
    /// [`AppState::set_extension_shortcuts`]). The binary calls this at boot from
    /// `ExtensionHost::shortcut_keys()`.
    pub fn set_extension_shortcuts(
        &mut self,
        specs: impl IntoIterator<Item = impl Into<ShortcutSpec>>,
    ) {
        self.state.set_extension_shortcuts(specs);
    }

    /// Plumb the `autocompleteMaxVisible` setting (Pi, item #6) into the editor's autocomplete popup
    /// (clamped 3–20). The binary calls this from `settings.autocompleteMaxVisible` at boot.
    pub fn set_autocomplete_max_visible(&mut self, n: u16) {
        self.state.editor.set_autocomplete_max_visible(n);
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
    /// ids in its own namespace (`app.*` / `editor.*` / `tui.select.*` / `app.tree.*`) and ignores the
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
        issues.extend(self.state.models_keymap.merge_json(json)?);
        issues.extend(self.state.editor.merge_keybindings_json(json)?);
        Ok(issues)
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
        self.state.models_keymap = crate::keymap::ModelsKeymap::default();
        self.state.editor.reset_keybindings_to_defaults();
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
        self.state.scrollback.iter().map(line_text).collect::<Vec<_>>().join("\n")
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
        self.paste_from_clipboard(read_clipboard_image_to_temp, crate::clipboard::read_clipboard_text)
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
        let caps = crate::image::detect_capabilities();
        self.state.capabilities = caps;
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
        self.state.image_renderer = ImageRenderer::from_capabilities_with_cell_size(caps, cell_size);
        // TUI-N01 / TUI-036 — publish the capability where the two consumers can reach it: the
        // transcript's tool-result image gate (Pi `tool-execution.ts:331`) and the `/settings` grid
        // builder, which must not offer image rows on a terminal with no protocol
        // (`settings-selector.ts:654-671`). `AppState::image_renderer` is not reachable from either.
        self.state
            .transcript
            .set_graphical_images(self.state.image_renderer.is_graphical());
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
