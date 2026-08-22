use super::*;

impl InputEditor {
    // ---- history ---------------------------------------------------------------------------

    /// Seed the prompt history with an already-submitted line — Pi's `editor.addToHistory?.(text)`
    /// on the `populateHistory` replay path (interactive-mode.ts:3387). Same skip rules as a live
    /// submission; call in chronological order so the newest replayed prompt ends up first.
    pub fn push_history(&mut self, text: &str) {
        self.add_to_history(text);
    }

    /// Add a raw submitted line to history (skip blank + consecutive-dup, `editor.ts:381-391`).
    pub(super) fn add_to_history(&mut self, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        if self.history.front().map(String::as_str) == Some(text) {
            return;
        }
        self.history.push_front(text.to_string());
        while self.history.len() > HISTORY_CAP {
            self.history.pop_back();
        }
    }

    /// Whether Up should enter / continue history browsing (`editor.ts:809-822`): at row 0 and the
    /// buffer is empty, already browsing, or the cursor is at col 0.
    pub(super) fn history_up_eligible(&self) -> bool {
        self.row == 0 && (self.is_empty() || self.history_index >= 0 || self.col == 0)
    }

    /// Older history entry (Up). On first entry, snapshot the draft.
    pub(super) fn history_older(&mut self) {
        if self.history.is_empty() {
            return;
        }
        if self.history_index < 0 {
            // `navigateHistory` (`editor.ts:435-438` @v0.83.0) pushes an undo snapshot *and* clones
            // the draft when browsing is first entered: `this.pushUndoSnapshot(); this.historyDraft =
            // structuredClone(this.state);`. cyrup saved the draft but never pushed the snapshot, so
            // Ctrl+- could not undo "I browsed history away from what I was typing".
            self.push_undo_for(LastAction::None);
            self.history_draft = Some(self.snapshot());
        }
        let next = (self.history_index + 1).min(self.history.len() as isize - 1);
        self.history_index = next;
        if let Some(entry) = self.history.get(next as usize).cloned() {
            // TUI-061 — `navigateHistory` uses `setTextInternal` (`editor.ts:1043-1056`), never
            // `setText`: browsing must not cancel the autocomplete, push a snapshot per step, or
            // clear the paste registry the draft is about to be restored with.
            self.set_text_internal(&entry);
            self.row = 0;
            self.col = 0; // cursor at start on Up (setTextInternal placement)
        }
    }

    /// Newer history entry (Down). Past the newest restores the saved draft.
    pub(super) fn history_newer(&mut self) {
        if self.history_index < 0 {
            return;
        }
        self.history_index -= 1;
        if self.history_index < 0 {
            // Restore the draft (`editor.ts:442-452`: `this.state = draft` + `preferredVisualCol =
            // null` + `scrollOffset = 0`, else `setTextInternal("")`). The draft reuses [`Snapshot`],
            // so it now carries the paste registry too: pi's `historyDraft` is a bare `EditorState`
            // (`:319`) only because nothing upstream mutates `pastes` while browsing — and nothing
            // here does either (every edit path calls `exit_history`, which drops the draft), so
            // restoring both fields is pi's outcome with the invariant made explicit rather than
            // assumed.
            if let Some(draft) = self.history_draft.take() {
                self.lines = draft.lines;
                self.row = draft.row.min(self.lines.len().saturating_sub(1));
                self.col = draft.col.min(self.cur_len());
                self.pastes = draft.pastes;
                self.paste_counter = draft.paste_counter;
                self.reset_preferred_col();
                self.scroll_offset = 0;
            } else {
                // `setTextInternal("")` (`:451`), NOT `clear()`: upstream's fallback empties the
                // buffer and leaves the paste registry, kill ring and popup state alone.
                self.set_text_internal("");
            }
        } else if let Some(entry) = self.history.get(self.history_index as usize).cloned() {
            // TUI-061 — the internal form again (`editor.ts:1043-1056`), same reason as the Up path.
            self.set_text_internal(&entry);
            self.move_end();
        }
    }

    /// Stop browsing history (any edit/newline/submit, `editor.ts`).
    pub(super) fn exit_history(&mut self) {
        self.history_index = -1;
        self.history_draft = None;
    }
}
