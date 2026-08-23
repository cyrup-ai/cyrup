use super::*;

impl InputEditor {
    // ---- key handling ----------------------------------------------------------------------

    /// Feed a key. Resolves an [`EditorAction`] via the keymap (R-10-018); printable chars insert.
    /// While a popup is open, navigation/accept/cancel route first (spec/tui/04 §5).
    pub fn handle_key(&mut self, ev: &KeyEvent) -> EditorOutcome {
        // 1. Char-jump mode consumes the next printable char (or cancels).
        if let Some(dir) = self.jump.take() {
            if let KeyCode::Char(c) = ev.code
                && !ev.modifiers.contains(KeyModifiers::CONTROL) {
                    self.jump_to(dir, c);
                    return EditorOutcome::Edited;
                }
            return EditorOutcome::Edited; // any other key cancels jump
        }

        // 2. Popup-open routing (before normal editing).
        if self.autocomplete.is_some()
            && let Some(outcome) = self.handle_popup_key(ev) {
                return outcome;
            }

        // 3. Resolve a bound editor action.
        if let Some(action) = self.keymap.action_for(ev) {
            return self.apply_editor_action(action);
        }

        // 4. Printable insert (no Ctrl/Super; Alt+char already routed via keymap or ignored).
        if let KeyCode::Char(c) = ev.code
            && !ev.modifiers.contains(KeyModifiers::CONTROL)
                && !ev.modifiers.contains(KeyModifiers::SUPER)
                && !ev.modifiers.contains(KeyModifiers::ALT)
            {
                self.push_undo_for_type(c);
                self.insert_char(c);
                self.last_action = LastAction::Type;
                self.exit_history();
                self.update_autocomplete();
                return EditorOutcome::Edited;
            }
        EditorOutcome::Ignored
    }

    /// Route a key while the popup is open through the configurable [`crate::AutocompleteKeymap`] (item #6 —
    /// the nav/accept/cancel keys are no longer hardcoded). Returns `Some` if consumed; `None` to
    /// fall through to normal editing.
    fn handle_popup_key(&mut self, ev: &KeyEvent) -> Option<EditorOutcome> {
        use crate::keymap::AutocompleteAction as A;
        match self.autocomplete_keymap.action_for(ev)? {
            A::Cancel => {
                self.autocomplete = None;
                Some(EditorOutcome::Edited)
            }
            A::Previous => {
                if let Some(ac) = self.autocomplete.as_mut() {
                    ac.list.select_up();
                }
                Some(EditorOutcome::Edited)
            }
            A::Next => {
                if let Some(ac) = self.autocomplete.as_mut() {
                    ac.list.select_down();
                }
                Some(EditorOutcome::Edited)
            }
            A::Accept => {
                // Accept, keep editing (no submit), then recompute (may close if out of context).
                self.accept_completion();
                self.update_autocomplete();
                Some(EditorOutcome::Edited)
            }
            A::AcceptSubmit => {
                let is_slash = self
                    .autocomplete
                    .as_ref()
                    .is_some_and(|ac| ac.context == CompletionContext::Slash);
                self.accept_completion();
                self.autocomplete = None;
                if is_slash {
                    // Accepting a slash item with Enter submits (spec/tui/04 §5, edge 15). The accept
                    // appended a trailing space; submit trims it (Pi trims in the submit handler).
                    let text = self.text().trim().to_string();
                    self.add_to_history(&text);
                    self.clear();
                    self.undo.clear(); // `this.undoStack.clear()` (`editor.ts:1268`)
                    Some(EditorOutcome::Submit(text))
                } else {
                    self.update_autocomplete();
                    Some(EditorOutcome::Edited)
                }
            }
        }
    }

    /// Dispatch a resolved editor action.
    fn apply_editor_action(&mut self, action: EditorAction) -> EditorOutcome {
        use EditorAction as E;
        // Any non-vertical action re-seeds the sticky goal column on the next Up/Down (spec/tui/03 §4.2).
        // `PageUp`/`PageDown` ARE vertical motion upstream — `pageScroll` shares `moveToVisualLine`
        // (and therefore `preferredVisualCol`) with `moveCursor` (`editor.ts:1373,1863`).
        if !matches!(action, E::CursorUp | E::CursorDown | E::PageUp | E::PageDown) {
            self.reset_preferred_col();
        }
        match action {
            E::CursorLeft => {
                self.move_left();
                self.last_action = LastAction::None;
                EditorOutcome::Edited
            }
            E::CursorRight => {
                self.move_right();
                self.last_action = LastAction::None;
                EditorOutcome::Edited
            }
            E::CursorUp => {
                // Every motion clears `lastAction` upstream — `moveCursor` (`editor.ts:1791`),
                // `navigateHistory` (`:430`), `moveWordBackwards`/`moveWordForwards` (`:1870`/`:2065`),
                // `moveToLineStart`/`moveToLineEnd`. cyrup cleared it on Left/Right only, so a kill
                // survived a vertical/word/line motion and the NEXT kill accumulated into the same
                // ring entry instead of pushing a new one (and a stale `Yank` still armed Alt+Y).
                self.last_action = LastAction::None;
                // History recall only fires on the first visual line (`history_up_eligible` already
                // requires `row == 0` + empty/browsing/col-0, which is always the first visual line).
                if self.history_up_eligible() {
                    self.history_older();
                } else {
                    self.move_up_visual();
                }
                EditorOutcome::Edited
            }
            E::CursorDown => {
                self.last_action = LastAction::None; // `editor.ts:1791` / `:430`
                if self.history_index >= 0 {
                    self.history_newer();
                } else {
                    self.move_down_visual();
                }
                EditorOutcome::Edited
            }
            // TUI-035 — `tui.editor.historyPrevious` / `historyNext`
            // (`tui/src/components/editor.ts:766-777` @v0.84.1). Upstream's comment is "Dedicated
            // history actions always browse entries instead of moving the cursor", and the two arms
            // sit AHEAD of the cursor-movement block: they cancel the autocomplete and call
            // `navigateHistory(∓1)` UNCONDITIONALLY, with none of the buffer-edge gating Up/Down
            // carry. Default `defaultKeys: []` (`keybindings.ts:68-75`), so nothing is bound until
            // the user says so — which is the point: they exist so Up/Down can be made pure caret
            // motion while history moves to, say, ctrl+p/ctrl+n.
            E::HistoryPrevious => {
                self.autocomplete = None;
                self.last_action = LastAction::None; // `navigateHistory` (`editor.ts:430`)
                self.history_older();
                EditorOutcome::Edited
            }
            E::HistoryNext => {
                self.autocomplete = None;
                self.last_action = LastAction::None;
                self.history_newer();
                EditorOutcome::Edited
            }
            // `tui.editor.pageUp` / `tui.editor.pageDown` (`editor.ts:855-862`): page the CARET
            // through the buffer. No history recall on either end (upstream's `pageScroll` never
            // touches `historyIndex`).
            E::PageUp => {
                self.page_scroll(-1);
                EditorOutcome::Edited
            }
            E::PageDown => {
                self.page_scroll(1);
                EditorOutcome::Edited
            }
            E::CursorWordLeft => {
                self.last_action = LastAction::None; // `moveWordBackwards`, `editor.ts:1870`
                self.move_word_left();
                EditorOutcome::Edited
            }
            E::CursorWordRight => {
                self.last_action = LastAction::None; // `moveWordForwards`, `editor.ts:2065`
                self.move_word_right();
                EditorOutcome::Edited
            }
            E::CursorLineStart => {
                self.last_action = LastAction::None; // `moveToLineStart`, `editor.ts:1783`
                self.move_home();
                EditorOutcome::Edited
            }
            E::CursorLineEnd => {
                self.last_action = LastAction::None; // `moveToLineEnd`, `editor.ts:1787`
                self.move_end();
                EditorOutcome::Edited
            }
            E::DeleteCharBackward => {
                self.push_undo_for(LastAction::None);
                self.backspace();
                self.last_action = LastAction::None;
                self.exit_history();
                self.update_autocomplete();
                EditorOutcome::Edited
            }
            E::DeleteCharForward => {
                self.push_undo_for(LastAction::None);
                self.delete();
                self.last_action = LastAction::None;
                self.update_autocomplete();
                EditorOutcome::Edited
            }
            E::DeleteWordBackward => {
                self.push_undo_for(LastAction::Kill);
                self.delete_word_backward();
                self.last_action = LastAction::Kill;
                self.update_autocomplete();
                EditorOutcome::Edited
            }
            E::DeleteWordForward => {
                self.push_undo_for(LastAction::Kill);
                self.delete_word_forward();
                self.last_action = LastAction::Kill;
                self.update_autocomplete();
                EditorOutcome::Edited
            }
            E::DeleteToLineStart => {
                self.push_undo_for(LastAction::Kill);
                self.delete_to_line_start();
                self.last_action = LastAction::Kill;
                self.update_autocomplete();
                EditorOutcome::Edited
            }
            E::DeleteToLineEnd => {
                self.push_undo_for(LastAction::Kill);
                self.delete_to_line_end();
                self.last_action = LastAction::Kill;
                EditorOutcome::Edited
            }
            E::Yank => {
                self.push_undo_for(LastAction::None);
                self.yank();
                self.last_action = LastAction::Yank;
                self.exit_history();
                EditorOutcome::Edited
            }
            E::YankPop => {
                self.yank_pop();
                self.last_action = LastAction::Yank;
                EditorOutcome::Edited
            }
            E::Undo => {
                self.undo();
                self.last_action = LastAction::None;
                self.update_autocomplete();
                EditorOutcome::Edited
            }
            E::NewLine => {
                self.push_undo_for(LastAction::None);
                self.insert_newline();
                self.last_action = LastAction::None;
                self.exit_history();
                EditorOutcome::Edited
            }
            E::Submit => {
                // Backslash-Enter → soft newline (Pi `editor.ts:796-802`, spec/tui/03 §5.7): a
                // workaround for terminals without Shift+Enter. If the char immediately before the
                // cursor is a literal backslash, delete it and insert a newline INSTEAD of submitting
                // (Pi `handleBackspace()` + `addNewLine()`), so `foo\<Enter>` breaks the line.
                if self.col > 0
                    && self.lines.get(self.row).and_then(|l| l.get(self.col - 1)) == Some(&'\\')
                {
                    self.push_undo_for(LastAction::None);
                    self.backspace();
                    self.insert_newline();
                    self.last_action = LastAction::None;
                    self.exit_history();
                    self.update_autocomplete();
                    return EditorOutcome::Edited;
                }
                // Expand large-paste markers back to their full content before the agent sees the text
                // (`expandPasteMarkers`, spec/tui/03 §5.5).
                let text = self.expanded_text();
                if text.trim().is_empty() {
                    return EditorOutcome::Edited;
                }
                self.add_to_history(&text);
                self.clear();
                // `submitValue` empties the undo stack with the buffer (`editor.ts:1268`), so
                // Ctrl+- after a send cannot resurrect the prompt that was just submitted.
                self.undo.clear();
                EditorOutcome::Submit(text)
            }
            E::Tab => self.trigger_completion(),
            E::JumpForward => {
                self.jump = Some(JumpDir::Forward);
                EditorOutcome::Edited
            }
            E::JumpBackward => {
                self.jump = Some(JumpDir::Backward);
                EditorOutcome::Edited
            }
        }
    }
}
