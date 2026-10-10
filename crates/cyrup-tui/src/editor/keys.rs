use super::*;

impl InputEditor {
    // ---- key handling ----------------------------------------------------------------------

    /// Feed a key. Resolves an [`EditorAction`] via the keymap (R-10-018); printable chars insert.
    /// While a popup is open, navigation/accept/cancel route first (spec/tui/04 §5).
    pub fn handle_key(&mut self, ev: &KeyEvent) -> EditorOutcome {
        // 1. Char-jump mode consumes the next printable char, cancels on the hotkey, or cancels and
        //    falls through on anything else — upstream's three arms, in order (`editor.ts:607-625`).
        if let Some(dir) = self.jump.take() {
            // (a) "Cancel if the hotkey is pressed again" (`:609-612`), with NO fall-through: the
            // keymap dispatch at step 3 resolves the same key back to `JumpForward`/`JumpBackward`,
            // so falling through here would RE-ARM jump mode instead of cancelling it.
            if matches!(
                self.keymap.action_for(ev),
                Some(EditorAction::JumpForward | EditorAction::JumpBackward)
            ) {
                return EditorOutcome::Edited;
            }
            // (b) Printable character — perform the jump (`:614-621`). Upstream's printable test is
            // `decodePrintableKey(data) ?? (data.charCodeAt(0) >= 32 ? data : undefined)`, which
            // rejects Alt+<char> because the terminal sends it as `ESC <char>` and the first byte is
            // 27; the ALT/SUPER exclusions here are that, matched to the insert path at step 4.
            if let KeyCode::Char(c) = ev.code
                && !ev.modifiers.contains(KeyModifiers::CONTROL)
                && !ev.modifiers.contains(KeyModifiers::ALT)
                && !ev.modifiers.contains(KeyModifiers::SUPER)
            {
                self.jump_to(dir, c);
                return EditorOutcome::Edited;
            }
            // (c) "Control character - cancel and fall through to normal handling" (`:623-624`).
            // The `take()` above already cleared jump mode, so the key now runs its own binding.
        }

        // 2. Popup-open routing (before normal editing).
        if self.autocomplete.is_some()
            && let Some(outcome) = self.handle_popup_key(ev)
        {
            return outcome;
        }

        // 3. Resolve a bound editor action.
        if let Some(action) = self.keymap.action_for(ev) {
            // `tui.input.copy` (`editor.ts:653-655`) is handled by a bare `return` upstream: the
            // editor declines the key and changes NOTHING — not even the sticky goal column, which
            // is why this returns ahead of `apply_editor_action`'s `reset_preferred_col` rather
            // than through its arm (TUI-067).
            if action == EditorAction::PassThrough {
                return EditorOutcome::Ignored;
            }
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
                //
                // DEVIATION, pre-existing and deliberate: upstream's Tab branch applies the
                // completion and then `cancelAutocomplete()` with NO recompute
                // (`components/editor.ts:676-698` @v0.84.3), so Tab-accepting `/model` leaves the
                // popup shut until the next keystroke, while cyrup immediately reopens it as the
                // argument popup. The recompute is what path completion drills directories with —
                // accepting `src/` and being offered its contents without a second Tab — so it is
                // kept, and kept GENERIC: not special-casing
                // [`crate::CompletionContext::SlashArgument`] out of it is the point, since a
                // per-context exception here is how the accept path drifts out of one rule into
                // four. The visible effect is new only because `SlashArgument` is now a reachable
                // context, not because the accept path changed.
                self.accept_completion();
                self.update_autocomplete();
                Some(EditorOutcome::Edited)
            }
            A::AcceptSubmit => {
                // Which accepted items fall through to submit. Upstream branches on ONE thing,
                // the OPEN POPUP'S PREFIX STRING (`components/editor.ts:806-813` @v1.1.0+11,
                // inside the `tui.select.confirm` arm at `:790-816`):
                //
                // ```ts
                // if (this.autocompletePrefix.startsWith("/")) {
                //     this.cancelAutocomplete();
                //     // Fall through to submit
                // } else {
                //     this.cancelAutocomplete();
                //     if (this.onChange) this.onChange(this.getText());
                //     return;
                // }
                // ```
                //
                // "Fall through" is mechanical, not figurative: `tui.select.confirm` and
                // `tui.input.submit` are BOTH bound to `enter` by default
                // (`keybindings.ts:154,144`), so the one key event that confirms also matches the
                // submit arm at `:909`, which calls `submitValue()` at `:921`. Note also that the
                // `if (selected && this.autocompleteProvider)` guard at `:792` does NOT return
                // when it fails, so Enter on a popup with no selected item submits upstream —
                // which cyrup matches, because a no-match popup's prefix is unchanged and
                // `accept_completion` is a no-op without a selection.
                //
                // TUI-057 — [CYRUP-DELTA]: cyrup tests the CONTEXT, not the prefix string, and
                // this is DELIBERATE. The comment that used to sit here claimed the two "agree by
                // construction on the slash surface"; that was wrong, and the correction does not
                // go the way it looks. They agree on the command-NAME popup — its prefix is the
                // whole `/…` token (`slash_context`'s `prefix: before.to_string()`, where `before`
                // is the `trim_start`ed command text, matching pi `autocomplete.ts:338,375`) and
                // `slash_context` returns `None` unless that text starts with `/`, so a `Slash`
                // popup's prefix ALWAYS starts with `/`. Porting pi's string test would therefore
                // not change this surface at all. What it WOULD change is every other popup whose
                // prefix can begin with `/` without being a command:
                //   - an ABSOLUTE-path popup (`cat /usr/lo<Tab>`, prefix `/usr/lo`, pi
                //     `autocomplete.ts:411`), and
                //   - a slash-ARGUMENT popup whose argument text itself starts with `/` (prefix is
                //     the bare argument, `/name ` already stripped — pi `:380,397`).
                // On both, pi fires the whole line at the agent on one Enter. That is upstream
                // being loose rather than upstream deciding: pi's OWN `applyCompletion` discriminates
                // the two cases properly, 120 lines away in the same file, with a three-part test
                // (`autocomplete.ts:433`):
                //
                // ```ts
                // const isSlashCommand = prefix.startsWith("/") && beforePrefix.trim() === "" && !prefix.slice(1).includes("/");
                // ```
                //
                // So pi has a precise "is a command name" predicate and simply does not use it at
                // `:806`. cyrup cannot adopt `:433` either: `!prefix.slice(1).includes("/")` is
                // false for cyrup's NAMESPACED command names (`/flux/aug`, a real registered shape
                // — `editor/tests/command_highlight.rs`'s `dynamic_commands_participate_in_both_rules`),
                // so it would stop `/flux/aug<Enter>` from submitting. `context == Slash` IS the
                // command-name popup, which is the set both upstream tests are reaching for, and it
                // is the only one of the three that is right about namespaced names. Keeping it
                // means an absolute-path popup needs a second Enter here and one upstream; that is
                // a deliberate narrowing of an accidental-submission hazard, pinned by
                // `enter_on_an_absolute_path_popup_does_not_submit`.
                let is_slash = self
                    .autocomplete
                    .as_ref()
                    .is_some_and(|ac| ac.context == CompletionContext::Slash);
                self.accept_completion();
                self.autocomplete = None;
                if is_slash {
                    // Accepting a slash item with Enter submits (spec/tui/04 §5, edge 15).
                    // TUI-057 — this is the fall-through into `submitValue()`, and upstream has
                    // exactly ONE submit path whose first act on the text is
                    // `expandPasteMarkers(this.state.lines.join("\n")).trim()`
                    // (`components/editor.ts:1366`). This arm called `self.text()` — the raw
                    // buffer join — so Enter on an open slash popup sent a literal
                    // `[paste #N …]` marker to the agent instead of the pasted content (reachable
                    // on one line: `/comp[paste #1 …]` with the cursor at col 5). The `.trim()`
                    // is the same `:1366` trim, and it is load-bearing beyond cosmetics: the
                    // accept rewrites the buffer to `/<name> ` WITH a trailing space (pi
                    // `autocomplete.ts:436`), and `CommandRegistry::dispatch` trims before
                    // `match_command`, so TUI-074's arity rule and this path stay coupled through
                    // "the submitted string is trimmed before dispatch". Do not drop this trim.
                    let text = self.expanded_text().trim().to_string();
                    self.add_to_history(&text);
                    self.clear();
                    self.undo.clear(); // `this.undoStack.clear()` (`editor.ts:1373`)
                    Some(EditorOutcome::Submit(text))
                } else {
                    // No recompute, unlike Tab's `Accept`: pi cancels and returns
                    // (`components/editor.ts:809-813` @v1.1.0+11), so the next Enter submits.
                    // Recomputing reopened the popup for a fully typed argument
                    // (`/login anthropic`), and every Enter re-accepted it without submitting.
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
        if !matches!(
            action,
            E::CursorUp | E::CursorDown | E::PageUp | E::PageDown
        ) {
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
                //
                // TUI-057 — the `.trim()` is upstream's, applied in the SAME expression as the
                // expansion: `const result = this.expandPasteMarkers(this.state.lines.join("\n")).trim();`
                // (`components/editor.ts:1366`), and `result` is what reaches BOTH `onSubmit` and
                // `addToHistory`. cyrup expanded but did not trim, while the `AcceptSubmit`
                // fall-through above DID — an asymmetry with a visible consequence in history:
                // `add_to_history`'s consecutive-dup check compares raw strings
                // (`editor/history.rs`), so submitting `/compact` and then `/compact ` left two
                // entries where pi leaves one. Command matching was never affected, because
                // `CommandRegistry::dispatch` trims before `match_command`.
                let text = self.expanded_text().trim().to_string();
                if text.is_empty() {
                    return EditorOutcome::Edited;
                }
                self.add_to_history(&text);
                self.clear();
                // `submitValue` empties the undo stack with the buffer (`editor.ts:1373`), so
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
            // Intercepted by [`Self::handle_key`] before it reaches here, so that a declined key
            // does not re-seed the goal column above. Kept as the same answer for a direct caller.
            E::PassThrough => EditorOutcome::Ignored,
        }
    }
}
