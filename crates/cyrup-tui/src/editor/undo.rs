use super::*;

/// Undo-stack bound — pi's `UndoStack` `maxSize` (`undo-stack.ts`), applied identically to the
/// editor's snapshots and to a single-line [`crate::text_input::Input`]'s `(value, cursor)` pairs.
pub(crate) const UNDO_CAP: usize = 500;

/// Pi's typing-coalescing rule, shared by `Editor.insertCharacter` (`editor.ts:1085-1094`) and
/// `Input.insertCharacter` (`components/input.ts:215`), which spell it identically:
///
/// ```text
/// if (isWhitespaceChar(char) || this.lastAction !== "type-word") this.pushUndo();
/// ```
///
/// Consecutive word characters coalesce into one undo unit, but a whitespace character *always*
/// captures the state before itself, so a single `Ctrl+-` removes the last word without swallowing
/// the whole line.
pub(crate) fn should_snapshot_for_type(c: char, last_was_type: bool) -> bool {
    c.is_whitespace() || !last_was_type
}

/// Push onto a bounded undo stack, dropping the oldest entry past `cap` so a long session does not
/// grow unbounded (pi's `UndoStack.push` shift, `undo-stack.ts`).
pub(crate) fn push_bounded<S>(stack: &mut Vec<S>, snap: S, cap: usize) {
    stack.push(snap);
    if stack.len() > cap {
        stack.remove(0);
    }
}

impl InputEditor {
    /// Snapshot the buffer + cursor **+ the paste registry** for undo — pi's `pushUndoSnapshot`
    /// payload `{ state, pastes, pasteCounter }` (`editor.ts:2012-2014` @v0.83.0), deep-copied by
    /// `structuredClone` upstream (`undo-stack.ts:11-13`) and by `Clone` here.
    pub(super) fn snapshot(&self) -> Snapshot {
        Snapshot {
            lines: self.lines.clone(),
            row: self.row,
            col: self.col,
            pastes: self.pastes.clone(),
            paste_counter: self.paste_counter,
        }
    }

    /// Push an undo snapshot, coalescing consecutive typing into one unit (fish-style,
    /// `editor.ts:1082-1095`): a run of `Type` actions shares a single snapshot.
    pub(super) fn push_undo_for(&mut self, action: LastAction) {
        let coalesce = action == LastAction::Type && self.last_action == LastAction::Type;
        if !coalesce {
            let snap = self.snapshot();
            push_bounded(&mut self.undo, snap, UNDO_CAP);
        }
    }

    /// Push an undo snapshot for a typed character, honoring Pi's fish-style **whitespace boundary**
    /// (`editor.ts:1085-1094`, item #5 "undo-whitespace coalescing"): consecutive word characters
    /// coalesce into one unit, but a whitespace character *always* captures the state before itself
    /// (`isWhitespaceChar(char) || lastAction !== "type-word"`), so a single `Ctrl+-` removes the last
    /// word without swallowing the whole line, and each space is a separate undo step.
    pub(super) fn push_undo_for_type(&mut self, c: char) {
        if should_snapshot_for_type(c, self.last_action == LastAction::Type) {
            let snap = self.snapshot();
            push_bounded(&mut self.undo, snap, UNDO_CAP);
        }
    }

    /// Restore the most recent undo snapshot (Ctrl+-). No redo (Pi parity). A statement-for-statement
    /// port of `undo()` (`editor.ts:2016-2030` @v0.83.0):
    ///
    /// ```text
    /// this.exitHistoryBrowsing();
    /// const snapshot = this.undoStack.pop();
    /// if (!snapshot) return;
    /// Object.assign(this.state, snapshot.state);
    /// this.pastes = snapshot.pastes;
    /// this.pasteCounter = snapshot.pasteCounter;
    /// this.lastAction = null;
    /// this.preferredVisualCol = null;
    /// ```
    ///
    /// `Object.assign(this.state, …)` restores **both** cursor coordinates (`EditorState` is
    /// `{ lines, cursorLine, cursorCol }`, `:209-213`) — cyrup used to keep the live `self.col` and
    /// merely clamp it, so the caret ended up wherever it happened to be and the next keystroke
    /// edited a position the user never chose (TUI-044). `min(cur_len())` is a bounds guard only.
    /// `last_action` is cleared by the [`EditorAction::Undo`] arm, matching `this.lastAction = null`.
    pub(super) fn undo(&mut self) {
        self.exit_history();
        if let Some(snap) = self.undo.pop() {
            self.lines = snap.lines;
            self.row = snap.row.min(self.lines.len().saturating_sub(1));
            self.col = snap.col.min(self.cur_len());
            self.pastes = snap.pastes;
            self.paste_counter = snap.paste_counter;
            self.reset_preferred_col();
        }
    }
}
