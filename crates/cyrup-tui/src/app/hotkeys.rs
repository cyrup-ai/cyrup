use super::*;

impl<B: Backend> App<B> {
    pub(crate) fn hotkeys_markdown(&self) -> String {
        let ek = self.state.editor.keymap_ref();
        let km = &self.state.keymap;
        // `keyDisplayText` — every bound key, `/`-joined, each part capitalized.
        let e = |a: EditorAction| {
            crate::chrome::format_key_text(&ek.keys_label(a).unwrap_or_default(), true)
        };
        let g =
            |a: Action| crate::chrome::format_key_text(&km.keys_label(a).unwrap_or_default(), true);
        let win_note = if cfg!(windows) {
            " (Ctrl+Enter on Windows Terminal)"
        } else {
            ""
        };
        let mut out = format!(
            "**Navigation**\n\
             | Key | Action |\n\
             |-----|--------|\n\
             | `{cursor_up}` / `{cursor_down}` / `{cursor_left}` / `{cursor_right}` | Move cursor / browse history |\n\
             | `{word_left}` / `{word_right}` | Move by word |\n\
             | `{line_start}` | Start of line |\n\
             | `{line_end}` | End of line |\n\
             | `{jump_fwd}` | Jump forward to character |\n\
             | `{jump_back}` | Jump backward to character |\n\
             | `{page_up}` / `{page_down}` | Scroll by page |\n\
             \n\
             **Editing**\n\
             | Key | Action |\n\
             |-----|--------|\n\
             | `{submit}` | Send message |\n\
             | `{new_line}` | New line{win_note} |\n\
             | `{del_word_back}` | Delete word backwards |\n\
             | `{del_word_fwd}` | Delete word forwards |\n\
             | `{del_line_start}` | Delete to start of line |\n\
             | `{del_line_end}` | Delete to end of line |\n\
             | `{yank}` | Paste the most-recently-deleted text |\n\
             | `{yank_pop}` | Cycle through the deleted text after pasting |\n\
             | `{undo}` | Undo |\n\
             \n\
             **Other**\n\
             | Key | Action |\n\
             |-----|--------|\n\
             | `{tab}` | Path completion / accept autocomplete |\n\
             | `{interrupt}` | Cancel autocomplete / abort streaming |\n\
             | `{clear}` | Clear editor (first) / exit (second) |\n\
             | `{exit}` | Exit (when editor is empty) |\n\
             | `{suspend}` | Suspend to background |\n\
             | `{thinking_cycle}` | Cycle thinking level |\n\
             | `{model_fwd}` / `{model_back}` | Cycle models |\n\
             | `{select_model}` | Open model selector |\n\
             | `{expand_tools}` | Toggle tool output expansion |\n\
             | `{toggle_thinking}` | Toggle thinking block visibility |\n\
             | `{external_editor}` | Edit message in external editor |\n\
             | `{copy_message}` | Copy last assistant message |\n\
             | `{follow_up}` | Queue follow-up message |\n\
             | `{dequeue}` | Restore queued messages |\n\
             | `{paste_image}` | Paste image or text from clipboard |\n\
             | `/` | Slash commands |\n\
             | `!` | Run bash command |\n\
             | `!!` | Run bash command (excluded from context) |",
            cursor_up = e(EditorAction::CursorUp),
            cursor_down = e(EditorAction::CursorDown),
            cursor_left = e(EditorAction::CursorLeft),
            cursor_right = e(EditorAction::CursorRight),
            word_left = e(EditorAction::CursorWordLeft),
            word_right = e(EditorAction::CursorWordRight),
            line_start = e(EditorAction::CursorLineStart),
            line_end = e(EditorAction::CursorLineEnd),
            jump_fwd = e(EditorAction::JumpForward),
            jump_back = e(EditorAction::JumpBackward),
            // Upstream reads these off the EDITOR map — `getEditorKeyDisplay("tui.editor.pageUp")`
            // (`interactive-mode.ts:5766-5767`, rendered at `:5808`) — not an app binding.
            page_up = e(EditorAction::PageUp),
            page_down = e(EditorAction::PageDown),
            submit = e(EditorAction::Submit),
            new_line = e(EditorAction::NewLine),
            del_word_back = e(EditorAction::DeleteWordBackward),
            del_word_fwd = e(EditorAction::DeleteWordForward),
            del_line_start = e(EditorAction::DeleteToLineStart),
            del_line_end = e(EditorAction::DeleteToLineEnd),
            yank = e(EditorAction::Yank),
            yank_pop = e(EditorAction::YankPop),
            undo = e(EditorAction::Undo),
            tab = e(EditorAction::Tab),
            interrupt = g(Action::Interrupt),
            clear = g(Action::Clear),
            exit = g(Action::Quit),
            suspend = g(Action::Suspend),
            thinking_cycle = g(Action::ThinkingCycle),
            model_fwd = g(Action::ModelCycleForward),
            model_back = g(Action::ModelCycleBackward),
            select_model = g(Action::ModelSelect),
            expand_tools = g(Action::ToolsExpand),
            toggle_thinking = g(Action::ThinkingToggle),
            external_editor = g(Action::ExternalEditor),
            copy_message = g(Action::MessageCopy),
            follow_up = g(Action::FollowUp),
            dequeue = g(Action::Dequeue),
            paste_image = g(Action::ClipboardPasteImage),
        );
        // `if (shortcuts.size > 0) { hotkeys += "\n**Extensions**\n| Key | Action |\n|-----|--------|\n" }`
        // then one `| \`key\` | description |` row per entry (`interactive-mode.ts:6189-6197`).
        // The key cell is `formatKeyText(key, { capitalize: true })` over the REGISTERED id, not a
        // keymap lookup — an extension shortcut is not a rebindable `Keybinding`, so there is
        // nothing to resolve it against.
        if !self.state.extension_shortcuts.is_empty() {
            out.push_str("\n\n**Extensions**\n| Key | Action |\n|-----|--------|");
            for (_, spec) in &self.state.extension_shortcuts {
                let key_display = crate::chrome::format_key_text(&spec.id, true);
                // `shortcut.description ?? shortcut.extensionPath` (`:6192`). cyrup's
                // `ExtensionHost::shortcut_keys()` currently surfaces neither field — the guest's
                // `register_shortcut(key, desc)` drops `desc` at `cyrup-ext/src/host/live.rs:98`
                // and the registry keys on `ExtensionId` — so with nothing registered the raw
                // key-id stands in. It identifies the shortcut truthfully; a fabricated label
                // would not.
                let label = spec.description.as_deref().unwrap_or(spec.id.as_str());
                out.push_str(&format!("\n| `{key_display}` | {label} |"));
            }
        }
        out
    }

    /// The `/hotkeys` body — Pi `handleHotkeysCommand` (interactive-mode.ts:6090-6205), verbatim: three
    /// `**Section**` headings each over a `| Key | Action |` GFM table, keys backticked and joined with
    /// ` / ` where a row names two bindings.
    ///
    /// Every cell is `keyDisplayText(id)` = `formatKeys(getKeys(id), { capitalize: true })`
    /// (`keybinding-hints.ts:29-39`), i.e. **all** keys bound to the id joined with `/` and each chord
    /// part title-cased — not just the first key, so a rebind that binds two keys shows both. Unbound
    /// ids render as the empty string exactly as upstream's `keys.length === 0 → ""` does (:30).
    ///
    /// The `**Other**` table is upstream's in full. It used to omit three rows behind a
    /// `[CYRUP-DELTA]` — `app.model.select`, `app.thinking.toggle`, `app.message.copy` — on the
    /// grounds that printing them with an empty key cell would advertise a shortcut no key reaches.
    /// That was legitimate only while the bindings were unported; **TUI-008 ported them**, so the
    /// rows are back at upstream's positions (`:5834`, `:5836`, `:5838`) and the delta is deleted
    /// rather than left to make `/hotkeys` permanently three rows short with nothing tracking it.
    ///
    /// The trailing **Extensions** table (`:6186-6197`) IS built. It is gated on
    /// `if (shortcuts.size > 0)` (`:6189`) — no registered shortcut, no section, never an empty
    /// table — and each row is
    /// ``| `${formatKeyText(key, { capitalize: true })}` | ${shortcut.description ?? shortcut.extensionPath} |``
    /// (`:6193-6197`). cyrup's registry is [`AppState::extension_shortcuts`], the very set the input
    /// router already matches presses against (`:1501`), fed from `ExtensionHost::shortcut_specs()`
    /// — so the section is a read of live state, not a fabricated list. EXT-040: it used to be fed
    /// from `shortcut_keys()`, a bare `Vec<String>`, so `description ?? extensionPath` always fell
    /// through to the id and every Action cell repeated its own Key cell.
    ///
    /// The `newLine` row's `" (Ctrl+Enter on Windows Terminal)"` suffix (:6151) is gated on
    /// `process.platform === "win32"`; it is emitted here under the same `cfg(windows)` condition.
    #[cfg(test)]
    pub(crate) fn hotkeys_markdown_for_test(&self) -> String {
        self.hotkeys_markdown()
    }
}
