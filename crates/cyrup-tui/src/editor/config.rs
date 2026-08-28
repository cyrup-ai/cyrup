use super::*;

impl Default for InputEditor {
    fn default() -> Self {
        InputEditor::new()
    }
}

impl InputEditor {
    /// A fresh, empty, focused editor.
    pub fn new() -> Self {
        InputEditor {
            lines: vec![Vec::new()],
            row: 0,
            col: 0,
            focused: true,
            keymap: EditorKeymap::default(),
            kill_ring: Vec::new(),
            last_action: LastAction::None,
            undo: Vec::new(),
            history: VecDeque::new(),
            history_index: -1,
            history_draft: None,
            jump: None,
            registry: CommandRegistry::new(),
            autocomplete: None,
            autocomplete_max_visible: crate::select_list::DEFAULT_MAX_VISIBLE,
            autocomplete_keymap: crate::keymap::AutocompleteKeymap::default(),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            mention_files: None,
            arg_sources: crate::autocomplete::ArgumentSources::default(),
            view_width: 80,
            scroll_offset: 0,
            term_rows: 24,
            preferred_visual_col: None,
            snapped_from_col: None,
            pastes: BTreeMap::new(),
            paste_counter: 0,
            thinking_level: "medium".to_string(),
            thinking_level_owns_border: true,
            padding_x: 0,
            show_hardware_cursor: false,
        }
    }

    /// Set the horizontal editor padding (Pi `CustomEditor.setPaddingX`,
    /// `tui/src/components/editor.ts:370-376`): non-finite → `0`, else `max(0, floor(padding))`.
    /// The `/settings` grid cycles `0..=3` and [`cyrup_config::SettingsManager::set_editor_padding_x`]
    /// clamps the persisted value to that range; this setter reproduces Pi's own coercion so a
    /// hand-edited `editorPaddingX` can never render negative or fractional padding.
    pub fn set_padding_x(&mut self, padding: i64) {
        self.padding_x = padding.clamp(0, u16::MAX as i64) as u16;
    }

    /// The current horizontal padding (Pi `getPaddingX`, `editor.ts:365-367`).
    #[must_use]
    pub fn padding_x(&self) -> u16 {
        self.padding_x
    }

    /// Show/hide the terminal's real cursor (Pi `TUI.setShowHardwareCursor`, `tui/src/tui.ts:346`).
    /// See [`Self::show_hardware_cursor`] for why this lives on the editor in cyrup.
    pub fn set_show_hardware_cursor(&mut self, enabled: bool) {
        self.show_hardware_cursor = enabled;
    }

    /// Whether the hardware cursor is placed each frame (Pi `getShowHardwareCursor`, `tui.ts:342`).
    ///
    /// Cyrup has no standalone `TUI` object owning terminal state — ratatui's `Terminal` does, and
    /// the ONLY component that asks for a cursor position is this editor
    /// ([`Self::cursor_in`] is the sole `Frame::set_cursor_position` caller in the crate), so the
    /// flag rides here rather than on a `TUI` port that does not exist.
    #[must_use]
    pub fn show_hardware_cursor(&self) -> bool {
        self.show_hardware_cursor
    }

    /// The padding actually applied at render width `width`, clamped exactly as Pi clamps it
    /// (`editor.ts:483-484`: `min(paddingX, floor((width - 1) / 2))`) so the content column can
    /// never be squeezed out of existence on a narrow terminal.
    pub fn effective_padding(&self, width: u16) -> u16 {
        self.padding_x.min(width.saturating_sub(1) / 2)
    }

    /// The text layout width inside `width` columns, after padding (Pi `editor.ts:485-489`):
    /// `contentWidth = max(1, width - 2 * paddingX)`, and one column is reserved for the
    /// end-of-line cursor cell ONLY when there is no padding for it to overflow into.
    ///
    /// **Public because measurement and render must agree** (E15). Pi computes `layoutWidth` ONCE
    /// per `render` and feeds the same number to `this.lastWidth` and to `layoutText()`
    /// (`editor.ts:489-497`), so the row count the container reserves is by construction the row
    /// count the editor draws. cyrup splits those two across `app::region_constraints` (which sizes
    /// the slot) and [`Component::render`] (which fills it), and the former used to measure at a
    /// hardcoded `width - 1` while the latter wrapped at `width - 2 * paddingX`: with
    /// `editorPaddingX > 0` the render produced MORE rows than the slot had, and the surplus — the
    /// caret row included — was clipped. Both sides now call this.
    pub fn layout_width(&self, width: u16) -> u16 {
        let pad = self.effective_padding(width);
        let content = width.saturating_sub(pad.saturating_mul(2)).max(1);
        if pad > 0 { content } else { content.saturating_sub(1).max(1) }
    }

    /// Set the reasoning level driving the editor's rule color (spec/tui/03 §3.3). Called by the app
    /// on `ThinkingLevelChanged` / thinking-selector confirm.
    pub fn set_thinking_level(&mut self, level: impl Into<String>) {
        self.thinking_level = level.into();
    }

    /// Detach this editor's rule from the reasoning level, leaving it on the `borderMuted` role —
    /// the state Pi's shared `Editor` is *born* in (`getEditorTheme().borderColor`, v0.84.1
    /// `theme.ts:1301-1304`) and never leaves unless something reassigns `borderColor`
    /// (`interactive-mode.ts:3990-3993`, which only ever touches the chat editor). Used by the
    /// `ui.editor` extension dialog, whose Pi counterpart is a bare
    /// `new Editor(tui, getEditorTheme(), options)` (`components/extension-editor.ts:70`).
    pub fn use_muted_border(&mut self) {
        self.thinking_level_owns_border = false;
    }

    /// Replace the command registry used for slash autocomplete (rebuilt on `/reload`).
    pub fn set_registry(&mut self, registry: CommandRegistry) {
        self.registry = registry;
    }

    /// The editor keymap (for `/hotkeys` to resolve editor-action key labels, `getEditorKeyDisplay`).
    pub fn keymap_ref(&self) -> &EditorKeymap {
        &self.keymap
    }

    /// Merge a JSON keybindings document into the editor keymap **and** the autocomplete-popup keymap
    /// (R-10-018; the `editor.*` + `tui.autocomplete.*` ids). Called by the binary at boot with the
    /// user's `keybindings.json` so custom editor + popup bindings take effect (item #6).
    /// CFG-038 — the two maps' rejected entries are CONCATENATED rather than short-circuited: the
    /// editor map returning an issue must not stop the autocomplete map from being applied.
    pub fn merge_keybindings_json(
        &mut self,
        json: &str,
    ) -> Result<Vec<crate::KeybindingIssue>, crate::TuiError> {
        let mut issues = self.keymap.merge_json(json)?;
        issues.extend(self.autocomplete_keymap.merge_json(json)?);
        Ok(issues)
    }

    /// Restore both editor-side binding tables to their defaults — TUI-051.
    ///
    /// `merge_json` only *sets* the ids present in the document, so a `/reload` after the user
    /// DELETED an entry would leave its old binding live. Upstream's `rebuild()` replaces rather
    /// than merges: for every id in `definitions`, `userKeys === undefined ?
    /// normalizeKeys(definition.defaultKeys) : normalizeKeys(userKeys)`
    /// (`packages/tui/src/keybindings.ts:187-191` @v0.83.0). Resetting to defaults and then merging
    /// the freshly-read document reproduces that.
    pub fn reset_keybindings_to_defaults(&mut self) {
        self.keymap = EditorKeymap::default();
        self.autocomplete_keymap = crate::keymap::AutocompleteKeymap::default();
    }

    /// Set the max visible rows in the autocomplete dropdown (Pi `autocompleteMaxVisible`, item #6):
    /// clamped to 3–20 and applied to any already-open popup + every future one. Called by the binary
    /// from `settings.autocompleteMaxVisible`.
    pub fn set_autocomplete_max_visible(&mut self, n: u16) {
        self.autocomplete_max_visible = n.clamp(3, 20);
        if let Some(ac) = self.autocomplete.as_mut() {
            ac.list.set_max_visible(self.autocomplete_max_visible);
        }
    }

    /// Install a freshly-computed autocomplete popup, applying the configured `autocompleteMaxVisible`
    /// dropdown height (item #6). Centralises the `self.autocomplete = Some(ac)` assignment so every
    /// entry point (slash / `@`-mention / forced Tab) honours the setting.
    pub(super) fn open_popup(&mut self, mut ac: Autocomplete) {
        ac.list.set_max_visible(self.autocomplete_max_visible);
        self.autocomplete = Some(ac);
    }

    /// Override the working directory used for path completion (defaults to the process cwd). Clears
    /// the cached `@`-mention file list so the new tree is re-enumerated on the next mention.
    pub fn set_cwd(&mut self, cwd: PathBuf) {
        self.cwd = cwd;
        self.mention_files = None;
    }

    /// Inject the `@`-mention candidate file list directly (test seam / an async populator). Bypasses
    /// the lazy `fd`/walk source.
    pub fn set_mention_files(&mut self, files: Vec<String>) {
        self.mention_files = Some(files);
    }

    /// Install the argument-completion sources for `/model`, `/login` and `/thinking`. Pi rebuilds
    /// the equivalent closures whenever it rebuilds the autocomplete provider
    /// (`createBaseAutocompleteProvider`, `interactive-mode.ts:677-736` @v0.84.3); cyrup pushes a
    /// snapshot instead, from [`crate::App::refresh_argument_sources`].
    ///
    /// `extension_completions` is deliberately CARRIED OVER rather than replaced: it is fed on a
    /// different clock (per keystroke, by [`crate::App::refresh_extension_completions`]) than the
    /// three builtin sets (boot / session swap / credential change / scope save), and the snapshot
    /// the caller builds has nothing to say about it. Overwriting it here would blank the popup
    /// whenever an unrelated refresh landed mid-argument.
    pub fn set_argument_sources(&mut self, sources: crate::autocomplete::ArgumentSources) {
        let extensions = std::mem::take(&mut self.arg_sources.extension_completions);
        self.arg_sources = sources;
        self.arg_sources.extension_completions = extensions;
    }

    /// Record what an extension command's own completer answered for `prefix`
    /// (`getArgumentCompletions`, `interactive-mode.ts:753` @v0.84.3), keyed by the command's
    /// invocation name. Pushed in by [`crate::App::refresh_extension_completions`], which is the
    /// async side of the seam described on [`crate::commands::ArgumentCompleter::Extension`].
    pub fn set_extension_completions(&mut self, command: &str, prefix: &str, items: Vec<String>) {
        self.arg_sources.extension_completions.insert(
            command.to_string(),
            crate::autocomplete::ExtensionCompletions {
                prefix: prefix.to_string(),
                items,
            },
        );
    }

    /// The `(command, argument)` pair under the cursor when the line being edited is
    /// `/<command> <argument>` and `<command>` is an extension command that declared its own
    /// completer — i.e. exactly when a guest call is worth making. `None` otherwise.
    ///
    /// This is [`crate::autocomplete::argument_completer`]'s resolution, reused so the async fetch
    /// and the sync popup can never disagree about which command owns the line.
    #[must_use]
    pub fn pending_extension_argument(&self) -> Option<(String, String)> {
        let before = self.before_cursor();
        let (completer, name, argument) =
            crate::autocomplete::argument_completer(&self.registry, &before)?;
        (completer == crate::commands::ArgumentCompleter::Extension)
            .then(|| (name.to_string(), argument.to_string()))
    }

    /// Recompute the popup from outside the key path, after new completion data arrived.
    pub fn refresh_autocomplete(&mut self) {
        self.update_autocomplete();
    }

    /// Focus state (R-10-009 — focused inputs drive the hardware cursor).
    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }
    pub fn is_focused(&self) -> bool {
        self.focused
    }

    /// Whether an autocomplete popup is currently open.
    pub fn autocomplete_open(&self) -> bool {
        self.autocomplete.is_some()
    }

    /// The active autocomplete popup, if any (for the chrome to render it below the editor).
    pub fn autocomplete(&self) -> Option<&Autocomplete> {
        self.autocomplete.as_ref()
    }

    /// True while the buffer's first line — **after leading whitespace** — begins with `!` (bash mode
    /// → green border, spec/tui/03 §7.1). Mirrors Pi's `text.trimStart().startsWith("!")`
    /// (interactive-mode.ts:2525): a leading indent before `!` (e.g. `  !ls`) still enters bash mode,
    /// matching the dispatcher, which `trim()`s before the `!` check (item #5 "trim_start on bash").
    pub fn is_bash_mode(&self) -> bool {
        self.lines
            .first()
            .and_then(|l| l.iter().find(|c| !c.is_whitespace()))
            .is_some_and(|c| *c == '!')
    }

    /// The full buffer text with `\n` line joins.
    pub fn text(&self) -> String {
        self.lines.iter().map(|l| l.iter().collect::<String>()).collect::<Vec<_>>().join("\n")
    }

    /// Whether the buffer is empty (no chars on any line).
    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(Vec::is_empty)
    }

    /// Number of buffer lines (≥ 1) — used to size the editor area.
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// The cursor position as `(row, col)` in chars.
    pub fn cursor(&self) -> (usize, usize) {
        (self.row, self.col)
    }

    /// The prompt history (most-recent first), for inspection/tests.
    pub fn history(&self) -> &VecDeque<String> {
        &self.history
    }

    /// The kill-ring top (most recent killed text), for inspection/tests.
    pub fn kill_ring_top(&self) -> Option<&str> {
        self.kill_ring.last().map(String::as_str)
    }

    /// Replace the buffer contents and move the cursor to the end.
    pub fn set_text(&mut self, text: &str) {
        // TUI-061 — Pi has TWO functions here and cyrup had collapsed them into one.
        // `setText` (`editor.ts:1010-1021` @v0.83.0) is the PROGRAMMATIC entry point:
        //
        // ```ts
        // this.cancelAutocomplete();
        // this.lastAction = null;
        // this.exitHistoryBrowsing();
        // const normalized = this.normalizeText(text);
        // if (this.getText() !== normalized) this.pushUndoSnapshot();   // makes it undoable
        // this.pastes.clear(); this.pasteCounter = 0;
        // this.setTextInternal(normalized);
        // ```
        //
        // `setTextInternal` (`:1043-1056`) — "Internal setText that doesn't reset history state -
        // used by navigateHistory" — does none of it. Collapsing the two left the paste registry
        // ALIVE across a programmatic buffer replacement (so a subsequently hand-typed
        // `[paste #1 …]` still expanded — TUI-049's surface, narrowed but not closed by that fix)
        // and made the replacement un-undoable.
        self.autocomplete = None;
        self.last_action = LastAction::None;
        self.exit_history();
        if self.text() != text {
            self.push_undo_for(LastAction::None);
        }
        self.pastes.clear();
        self.paste_counter = 0;
        self.set_text_internal(text);
    }

    /// Pi's `setTextInternal` (`editor.ts:1043-1056` @v0.83.0) — "Internal setText that doesn't
    /// reset history state - used by navigateHistory". No autocomplete cancel, no undo snapshot, no
    /// registry reset: the buffer is replaced and the scroll re-anchored, nothing else. TUI-061.
    pub(super) fn set_text_internal(&mut self, text: &str) {
        self.lines = text.split('\n').map(|l| l.chars().collect()).collect();
        if self.lines.is_empty() {
            self.lines.push(Vec::new());
        }
        self.row = self.lines.len().saturating_sub(1);
        self.col = self.lines.get(self.row).map_or(0, Vec::len);
        // `editor.ts:471`: "Reset scroll - render() will adjust to show cursor".
        self.scroll_offset = 0;
    }

    /// Clear the buffer back to a single empty line and reset transient state.
    pub fn clear(&mut self) {
        self.lines = vec![Vec::new()];
        self.row = 0;
        self.col = 0;
        self.autocomplete = None;
        self.jump = None;
        self.last_action = LastAction::None;
        // Both sticky-column fields, not just the preferred one: this method stands in for
        // `submitValue`/`setText`, after which a `snapped_from_col` left over from the old buffer
        // would misresolve the first vertical move in the new one (`setCursorCol`,
        // `editor.ts:1377-1381`).
        self.reset_preferred_col();
        self.scroll_offset = 0;
        self.pastes.clear();
        // `this.pastes.clear(); this.pasteCounter = 0;` — both halves, on both upstream paths this
        // method stands in for (`submitValue`, `editor.ts:1264-1266`; `setText`, `:1018-1020`).
        // Clearing the map without resetting the counter left paste ids drifting up for the life of
        // the session, so cyrup's `[paste #7 …]` was pi's `[paste #1 …]`.
        self.paste_counter = 0;
        self.exit_history();
    }

    /// The char length of the current line (0 if somehow out of range).
    pub(super) fn cur_len(&self) -> usize {
        self.lines.get(self.row).map_or(0, Vec::len)
    }
}
