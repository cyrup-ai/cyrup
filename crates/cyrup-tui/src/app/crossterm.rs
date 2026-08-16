use super::*;

/// Resolve the external-editor command for the live session honoring Pi's precedence — settings
/// `externalEditor` → `$VISUAL` → `$EDITOR` → platform default (F14; Pi `getExternalEditorCommand`,
/// settings-manager.ts:846-848, consulted by `openExternalEditor` extension-editor.ts:117). Delegates
/// to the settings-tested [`cyrup_config::EffectiveSettings::external_editor`] (re-exported as
/// [`cyrup_session_svc::EffectiveSettings`]) so a configured editor is honored over the environment,
/// instead of the old inline `$VISUAL`/`$EDITOR`-only chain that ignored it.
pub(crate) fn resolve_external_editor(session: &AgentSession) -> String {
    session
        .services()
        .settings
        .effective()
        .external_editor(&cyrup_session_svc::EnvVars::from_process())
}

/// Spawn `editor_cmd path` (inheriting stdio) and, on a clean exit, return the file's contents with a
/// single trailing newline stripped (Pi's "reload the edited text"); `None` on a non-zero exit / spawn
/// failure (Pi's "no change"). `editor_cmd` is split on whitespace so `code --wait`-style commands work,
/// with `path` appended as the final argument. Pure (no terminal teardown, no `self`) so the resolved
/// command that actually runs can be exercised directly by a test — the terminal suspend/restore is the
/// caller's ([`App::edit_in_external_editor`]) responsibility.
pub(crate) fn run_editor_over_file(editor_cmd: &str, path: &std::path::Path) -> Option<String> {
    let mut parts = editor_cmd.split_whitespace();
    let status = parts
        .next()
        .map(|bin| std::process::Command::new(bin).args(parts).arg(path).status());
    if let Some(Ok(s)) = status
        && s.success()
        && let Ok(new_text) = std::fs::read_to_string(path)
    {
        let trimmed = new_text.strip_suffix('\n').unwrap_or(&new_text);
        return Some(trimmed.to_string());
    }
    None
}

impl App<CrosstermBackend<Stdout>> {
    /// Build the production app: raw mode on, bracketed paste + Kitty keyboard flags enabled
    /// (best-effort, with graceful fallback, R-ARCH-TUI-008), inline viewport on stdout.
    ///
    /// The panic hook goes in FIRST, before a single terminal mode is touched, so the window it
    /// covers is a superset of the window that can leave the terminal broken — a panic between
    /// `enable_raw_mode` and the return of this function is exactly as fatal to the user's shell as
    /// one during the event loop. Ports pi's `uncaughtCrash` install
    /// (`interactive-mode.ts:3684-3686`, handler at `:3622-3638`).
    pub fn into_stdout(theme: UiTheme) -> Result<Self, TuiError> {
        crate::panic_hook::install_panic_hook();
        enable_raw_mode()?;
        let mut out = io::stdout();
        out.execute(ratatui::crossterm::event::EnableBracketedPaste)?;
        // Kitty keyboard protocol where supported; ignore failure (legacy terminals).
        let _ = execute!(
            out,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        );
        // …and then ASK whether the push took, instead of assuming it did (Pi
        // `queryAndEnableKittyProtocol`, `tui/src/terminal.ts:213-226`). The query has to follow the
        // push — `CSI ? u` reports the top of the terminal's flag stack — and it has to run HERE:
        // this is the one window where raw mode is on and no crossterm reader thread is competing
        // for the reply (see `crate::keyboard_protocol`'s module docs, and
        // `crate::terminal_query`'s for the read's timeout/input-safety contract). The recorded
        // outcome is what the re-entry paths below re-apply and what the startup diagnostics read.
        let _ = crate::keyboard_protocol::negotiate();
        App::new(CrosstermBackend::new(out), theme)
    }

    /// Draw one frame wrapped in synchronized-output markers (CSI 2026, R-10-002 / R-ARCH-TUI-004).
    pub fn draw_synchronized(&mut self) -> Result<(), TuiError> {
        // The OSC 9;4 write for a progress transition the session-event fold (or a `/settings` flip)
        // recorded. Pi writes it synchronously inside the event handler
        // (`interactive-mode.ts:2865-2867` → `terminal.ts:509-523`); cyrup's fold is a pure state
        // transition, so the write happens here — one call site, ahead of the frame, reached by
        // EVERY run-loop arm that can have changed the state. Draining makes it once-per-transition
        // rather than once-per-frame.
        self.flush_terminal_progress();
        let mut out = io::stdout();
        let _ = out.execute(BeginSynchronizedUpdate);
        let res = self.draw();
        let _ = out.execute(EndSynchronizedUpdate);
        res
    }

    /// Suspend the process to the background (Ctrl+Z / `app.suspend`, `core/keybindings.ts`): tear the
    /// terminal back down to a usable cooked state, raise `SIGTSTP` on our own process group so the
    /// shell regains control, then — when the user `fg`s us and the kernel delivers `SIGCONT` — restore
    /// raw mode + the inline viewport and redraw. The signal is raised by shelling out to `kill -s
    /// TSTP <pid>` so the crate stays `#![forbid(unsafe_code)]` with **no** new dependency (a libc
    /// `raise` would need an unsafe shim + a new dep; the `kill` path needs neither). Unix-only; on
    /// other platforms it degrades to a redraw.
    pub fn suspend(&mut self) -> Result<(), TuiError> {
        // TUI-092 — announce a BY-DESIGN block to the input reader's wedge detector for the whole
        // of this call. The run loop stops servicing input here until the user `fg`s us, which is
        // indistinguishable from a wedge by observation alone; the flag is what tells the reader
        // not to escalate a working `Ctrl+Z` into an app exit. Held across the SIGTSTP and the
        // re-entry below, and dropped only once raw mode and the viewport are back.
        let _released = TerminalReleased::enter();
        self.restore()?;
        #[cfg(unix)]
        {
            // Stop our own process group; `kill` exits before the stop takes effect, and we resume on
            // SIGCONT (shell `fg`) at the next statement.
            let pid = std::process::id().to_string();
            let _ = std::process::Command::new("kill").args(["-s", "TSTP", &pid]).status();
        }
        // Resumed (or non-unix): re-enter raw mode + flags, then redraw the live region. The flags
        // are re-pushed unconditionally, exactly as Pi's `start()` does (`terminal.ts:164-166`) —
        // NOT re-negotiated: the crossterm reader thread is live by now, so a `CSI ? u` reply would
        // race it (`crate::keyboard_protocol` module docs). The startup decision still stands.
        enable_raw_mode()?;
        let mut out = io::stdout();
        let _ = out.execute(ratatui::crossterm::event::EnableBracketedPaste);
        let _ = execute!(
            out,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        );
        let _ = self.terminal.clear();
        self.draw_synchronized()
    }

    /// Open the editor buffer in an external editor (Ctrl+G / `app.editor.external`,
    /// `openExternalEditor` interactive-mode.ts:3611): run the caller-resolved `editor_cmd` (the
    /// settings `externalEditor` → `$VISUAL` → `$EDITOR` → default chain, see [`App::run`]), write the
    /// buffer to a temp `*.pi.md`, tear the TUI down to release the terminal, run the editor (inheriting
    /// stdio), and — on a clean exit — reload the edited text (trailing newline stripped). The terminal
    /// is always restored, even on error. No `unsafe`, no new dependency (`std::process` + `std::fs`).
    pub fn open_external_editor(&mut self, editor_cmd: &str) -> Result<(), TuiError> {
        let current = self.state.editor.text();
        if let Some(new_text) = self.edit_in_external_editor(&current, editor_cmd)? {
            self.state.editor.set_text(&new_text);
        }
        self.draw_synchronized()
    }

    /// `Ctrl+G` pressed inside the extension `ui.editor` dialog (L4 review §3;
    /// [`AppAction::OpenExternalEditorForSelector`]): seed `$VISUAL`/`$EDITOR` with the OPEN
    /// dialog's own buffer (never [`AppState::editor`], the live prompt draft — unrelated) and, on a
    /// clean exit, write the result back into the SAME dialog buffer via
    /// [`Selector::apply_external_edit`] — the dialog stays open (Pi never resolves it from this
    /// path, `extension-editor.ts:119-157`); only `Enter`/`Esc` close it. A no-op if no selector is
    /// open or the open one doesn't support external editing (`external_edit_text` returns `None`).
    pub(crate) fn open_external_editor_for_selector(&mut self, editor_cmd: &str) -> Result<(), TuiError> {
        let Some(current) = self.state.selector.as_ref().and_then(|a| a.inner.external_edit_text())
        else {
            return Ok(());
        };
        if let Some(new_text) = self.edit_in_external_editor(&current, editor_cmd)?
            && let Some(active) = self.state.selector.as_mut()
        {
            active.inner.apply_external_edit(&new_text);
        }
        self.draw_synchronized()
    }

    /// Run the resolved `editor_cmd` over `initial` text and return the edited result on a clean exit
    /// (`Ok(None)` on a non-zero exit / spawn failure / unwritable temp file — Pi's "no change"). Tears
    /// the TUI down for the duration and always restores it before returning, even on failure — the
    /// caller is left with a usable terminal either way.
    ///
    /// `editor_cmd` is resolved by the caller (`App::run`) through the SAME precedence Pi uses —
    /// settings `externalEditor` → `$VISUAL` → `$EDITOR` → platform default
    /// ([`cyrup_config::EffectiveSettings::external_editor`], settings-manager.ts:846,
    /// extension-editor.ts:117) — rather than the old inline `$VISUAL`/`$EDITOR`-only chain that
    /// silently ignored a configured `externalEditor` (F14). This method just SPAWNS the resolved
    /// command via [`run_editor_over_file`].
    ///
    /// The synchronous, TUI-suspending core both [`Self::open_external_editor`] (Ctrl+G on the live
    /// input buffer) and [`Self::open_external_editor_for_selector`] (Ctrl+G inside the extension
    /// `ui.editor` dialog, L4 review §3) share.
    ///
    /// This runs entirely synchronously on the caller's task (no `.await`) — reused directly inside
    /// `App::run`'s `select!` loop is safe (nothing here can deadlock against a concurrently-blocked
    /// guest, unlike the `execute_command`/`run_shortcut` paths, which must never await guest-reentrant
    /// work inline for exactly that reason; see `App::run`'s `AppAction::ExtensionShortcut` arm).
    fn edit_in_external_editor(
        &mut self,
        initial: &str,
        editor_cmd: &str,
    ) -> Result<Option<String>, TuiError> {
        // TUI-092 — a BY-DESIGN block, exactly as in [`Self::suspend`]: `run_editor_over_file` is a
        // blocking `Command::status()` that can own the terminal for minutes, during which the run
        // loop services no input. The flag stops the input reader's wedge detector from escalating
        // a `Ctrl+C` typed inside `$EDITOR` into an app exit. Taken FIRST so the early `return
        // Ok(None)` below is covered too, and dropped by `Drop` on every exit path.
        let _released = TerminalReleased::enter();
        let mut tmp = std::env::temp_dir();
        tmp.push(format!("cyrup-editor-{}.pi.md", std::process::id()));
        if std::fs::write(&tmp, initial).is_err() {
            self.state.transcript.push_status("external editor: could not write temp file");
            return Ok(None);
        }

        // Release the terminal (cooked mode, no inline viewport) so the editor owns the screen.
        self.restore()?;
        let result = run_editor_over_file(editor_cmd, &tmp);
        let _ = std::fs::remove_file(&tmp);

        // Re-enter raw mode + bracketed paste + Kitty flags; the caller redraws. Re-pushed, never
        // re-negotiated — same reason as `suspend` above.
        enable_raw_mode()?;
        let mut out = io::stdout();
        let _ = out.execute(ratatui::crossterm::event::EnableBracketedPaste);
        let _ = execute!(
            out,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        );
        let _ = self.terminal.clear();
        Ok(result)
    }
}
