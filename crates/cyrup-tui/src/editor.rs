//! The multi-line input editor (spec/tui/03; arch-10 §3.6; gaps 23-31, 36).
//!
//! A char-indexed `Vec<Vec<char>>` buffer (one inner vec per logical line) so all cursor math is on
//! `char` indices and never byte-indexes UTF-8 (no-panic policy, R-00-009). Beyond insert/motion the
//! editor ports Pi's editor surface (`pi-tui/src/components/editor.ts`): word navigation, the kill
//! ring (Ctrl+W/U/K, Alt+D feed it; Ctrl+Y yanks; Alt+Y yank-pops), an undo stack (Ctrl+-) with
//! typing coalescing, prompt history recall (Up/Down at the buffer edges, 100-entry, draft save),
//! char-jump (Ctrl+]), bash-mode detection (leading `!`/`!!`), and the slash/path autocomplete popup
//! (`autocomplete.rs`). Keys resolve through [`EditorKeymap`] — the editor never compares keys inline
//! (R-10-018). Grapheme-cluster motion (emoji/combining) and wrap-aware vertical motion remain a
//! tracked residual (char-granular here).

use std::collections::VecDeque;
use std::path::PathBuf;

use ratatui::layout::Rect;
use ratatui::symbols::border;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::autocomplete::{Autocomplete, CompletionContext};
use crate::commands::CommandRegistry;
use crate::component::Component;
use crate::keymap::{EditorAction, EditorKeymap};
use crate::theme::UiTheme;

/// History ring capacity (`editor.ts:381`).
const HISTORY_CAP: usize = 100;

/// Outcome of feeding a key to the editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorOutcome {
    /// Enter pressed (or a slash-command accepted from the popup): the submitted text (buffer cleared).
    Submit(String),
    /// The buffer or cursor changed → request a re-render.
    Edited,
    /// The key was not handled by the editor.
    Ignored,
}

/// What the previous mutating op was, gating kill-ring accumulation and undo coalescing
/// (`editor.ts` `lastAction`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LastAction {
    None,
    Kill,
    Yank,
    Type,
}

/// Direction for char-jump mode (`editor.ts:307`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JumpDir {
    Forward,
    Backward,
}

/// An undo snapshot (deep clone of buffer + cursor, `undo-stack.ts`).
#[derive(Clone, Debug)]
struct Snapshot {
    lines: Vec<Vec<char>>,
    row: usize,
    col: usize,
}

/// A multi-line text editor with a block cursor, kill ring, undo, history, and autocomplete.
pub struct InputEditor {
    /// Lines as char vectors. Invariant: always at least one line.
    lines: Vec<Vec<char>>,
    row: usize,
    col: usize,
    focused: bool,
    keymap: EditorKeymap,
    kill_ring: Vec<String>,
    last_action: LastAction,
    undo: Vec<Snapshot>,
    history: VecDeque<String>,
    /// `-1` ⇒ not browsing history; otherwise an index into `history`.
    history_index: isize,
    history_draft: Option<Snapshot>,
    jump: Option<JumpDir>,
    registry: CommandRegistry,
    autocomplete: Option<Autocomplete>,
    cwd: PathBuf,
}

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
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        }
    }

    /// Replace the command registry used for slash autocomplete (rebuilt on `/reload`).
    pub fn set_registry(&mut self, registry: CommandRegistry) {
        self.registry = registry;
    }

    /// Override the working directory used for path completion (defaults to the process cwd).
    pub fn set_cwd(&mut self, cwd: PathBuf) {
        self.cwd = cwd;
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

    /// True while the buffer text begins with `!` (bash mode → green border, spec/tui/03 §7.1).
    pub fn is_bash_mode(&self) -> bool {
        self.lines.first().is_some_and(|l| l.first() == Some(&'!'))
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
        self.lines = text.split('\n').map(|l| l.chars().collect()).collect();
        if self.lines.is_empty() {
            self.lines.push(Vec::new());
        }
        self.row = self.lines.len().saturating_sub(1);
        self.col = self.lines.get(self.row).map_or(0, Vec::len);
    }

    /// Clear the buffer back to a single empty line and reset transient state.
    pub fn clear(&mut self) {
        self.lines = vec![Vec::new()];
        self.row = 0;
        self.col = 0;
        self.autocomplete = None;
        self.jump = None;
        self.last_action = LastAction::None;
        self.exit_history();
    }

    /// The char length of the current line (0 if somehow out of range).
    fn cur_len(&self) -> usize {
        self.lines.get(self.row).map_or(0, Vec::len)
    }

    /// Snapshot the buffer + cursor for undo.
    fn snapshot(&self) -> Snapshot {
        Snapshot { lines: self.lines.clone(), row: self.row, col: self.col }
    }

    /// Push an undo snapshot, coalescing consecutive typing into one unit (fish-style,
    /// `editor.ts:1082-1095`): a run of `Type` actions shares a single snapshot.
    fn push_undo_for(&mut self, action: LastAction) {
        let coalesce = action == LastAction::Type && self.last_action == LastAction::Type;
        if !coalesce {
            self.undo.push(self.snapshot());
            // Bound the stack so a long session does not grow unbounded.
            if self.undo.len() > 500 {
                self.undo.remove(0);
            }
        }
    }

    /// Restore the most recent undo snapshot (Ctrl+-). No redo (Pi parity, `editor.ts:1974-1984`).
    fn undo(&mut self) {
        if let Some(snap) = self.undo.pop() {
            self.lines = snap.lines;
            self.row = snap.row.min(self.lines.len().saturating_sub(1));
            self.col = self.col.min(self.cur_len());
            self.exit_history();
        }
    }

    // ---- insertion -------------------------------------------------------------------------

    /// Insert a printable character at the cursor.
    pub fn insert_char(&mut self, c: char) {
        let col = self.col.min(self.cur_len());
        if let Some(line) = self.lines.get_mut(self.row) {
            line.insert(col, c);
            self.col = col + 1;
        }
    }

    /// Insert a string (e.g. a paste) char by char.
    pub fn insert_str(&mut self, s: &str) {
        self.push_undo_for(LastAction::None);
        for c in s.chars() {
            if c == '\n' {
                self.insert_newline();
            } else {
                self.insert_char(c);
            }
        }
        self.last_action = LastAction::None;
        self.exit_history();
        self.update_autocomplete();
    }

    /// Split the current line at the cursor (newline).
    pub fn insert_newline(&mut self) {
        let col = self.col.min(self.cur_len());
        let tail = match self.lines.get_mut(self.row) {
            Some(line) => line.split_off(col),
            None => Vec::new(),
        };
        let next = self.row + 1;
        self.lines.insert(next, tail);
        self.row = next;
        self.col = 0;
    }

    // ---- deletion --------------------------------------------------------------------------

    /// Backspace: delete the char before the cursor, joining lines at column 0.
    pub fn backspace(&mut self) {
        if self.col > 0 {
            let idx = self.col - 1;
            if let Some(line) = self.lines.get_mut(self.row)
                && idx < line.len() {
                    line.remove(idx);
                }
            self.col = idx;
        } else if self.row > 0 && self.row < self.lines.len() {
            let cur = self.lines.remove(self.row);
            let prev_row = self.row - 1;
            if let Some(prev) = self.lines.get_mut(prev_row) {
                let join = prev.len();
                prev.extend(cur);
                self.row = prev_row;
                self.col = join;
            }
        }
    }

    /// Forward-delete: delete the char at the cursor, joining the next line at end-of-line.
    pub fn delete(&mut self) {
        let len = self.cur_len();
        if self.col < len {
            if let Some(line) = self.lines.get_mut(self.row)
                && self.col < line.len() {
                    line.remove(self.col);
                }
        } else if self.row + 1 < self.lines.len() {
            let next = self.lines.remove(self.row + 1);
            if let Some(line) = self.lines.get_mut(self.row) {
                line.extend(next);
            }
        }
    }

    /// Delete from the word-left boundary to the cursor, feeding the kill ring (Ctrl+W,
    /// `editor.ts:1479`). Coalesces with a preceding kill (prepend).
    fn delete_word_backward(&mut self) {
        let target = self.word_left_target();
        if target == (self.row, self.col) {
            return;
        }
        let killed = self.take_range(target, (self.row, self.col));
        self.push_kill(&killed, false);
        self.row = target.0;
        self.col = target.1;
    }

    /// Delete from the cursor to the word-right boundary, feeding the kill ring (Alt+D).
    fn delete_word_forward(&mut self) {
        let target = self.word_right_target();
        if target == (self.row, self.col) {
            return;
        }
        let killed = self.take_range((self.row, self.col), target);
        self.push_kill(&killed, true);
    }

    /// Delete from line start to the cursor, feeding the kill ring (Ctrl+U).
    fn delete_to_line_start(&mut self) {
        if self.col == 0 {
            return;
        }
        let killed = self.take_range((self.row, 0), (self.row, self.col));
        self.push_kill(&killed, false);
        self.col = 0;
    }

    /// Delete from the cursor to line end, feeding the kill ring (Ctrl+K).
    fn delete_to_line_end(&mut self) {
        let len = self.cur_len();
        if self.col >= len {
            return;
        }
        let killed = self.take_range((self.row, self.col), (self.row, len));
        self.push_kill(&killed, true);
    }

    /// Remove and return the text between two same-or-adjacent positions on the current line.
    /// (Multi-line kills are out of scope here; ranges are within one logical line.)
    fn take_range(&mut self, start: (usize, usize), end: (usize, usize)) -> String {
        if start.0 != end.0 {
            return String::new();
        }
        let Some(line) = self.lines.get_mut(start.0) else { return String::new() };
        let lo = start.1.min(line.len());
        let hi = end.1.min(line.len());
        if lo >= hi {
            return String::new();
        }
        let drained: String = line.drain(lo..hi).collect();
        drained
    }

    /// Push killed text onto the ring, accumulating into the top entry when the previous action was
    /// also a kill (prepend for backward kills, append for forward kills; `kill-ring.ts`).
    fn push_kill(&mut self, text: &str, append: bool) {
        if self.last_action == LastAction::Kill
            && let Some(top) = self.kill_ring.last_mut() {
                if append {
                    top.push_str(text);
                } else {
                    *top = format!("{text}{top}");
                }
                return;
            }
        self.kill_ring.push(text.to_string());
    }

    /// Yank the kill-ring top at the cursor (Ctrl+Y, `editor.ts:1852`).
    fn yank(&mut self) {
        if let Some(top) = self.kill_ring.last().cloned() {
            for c in top.chars() {
                if c == '\n' {
                    self.insert_newline();
                } else {
                    self.insert_char(c);
                }
            }
        }
    }

    /// Yank-pop: only after a yank with ≥2 ring entries — delete the just-yanked text, rotate the
    /// ring, and insert the new top (Alt+Y, `editor.ts:1867`).
    fn yank_pop(&mut self) {
        if self.last_action != LastAction::Yank || self.kill_ring.len() < 2 {
            return;
        }
        // Delete the previously-yanked text (the current ring top) backward from the cursor.
        if let Some(prev) = self.kill_ring.last().cloned() {
            let n = prev.chars().count();
            for _ in 0..n {
                self.backspace();
            }
        }
        // Rotate: move the top to the front (so a fresh top becomes current).
        if let Some(top) = self.kill_ring.pop() {
            self.kill_ring.insert(0, top);
        }
        if let Some(top) = self.kill_ring.last().cloned() {
            for c in top.chars() {
                self.insert_char(c);
            }
        }
    }

    // ---- motion ----------------------------------------------------------------------------

    pub fn move_left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.cur_len();
        }
    }

    pub fn move_right(&mut self) {
        if self.col < self.cur_len() {
            self.col += 1;
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
    }

    pub fn move_home(&mut self) {
        self.col = 0;
    }

    pub fn move_end(&mut self) {
        self.col = self.cur_len();
    }

    /// The word-left target `(row, col)` (`word-navigation.ts`): skip a whitespace run, then consume
    /// one word/punctuation segment; honor punctuation sub-boundaries inside a word. At col 0 step to
    /// the previous line's end.
    fn word_left_target(&self) -> (usize, usize) {
        let Some(line) = self.lines.get(self.row) else { return (self.row, self.col) };
        let mut i = self.col;
        if i == 0 {
            if self.row > 0 {
                let prev_len = self.lines.get(self.row - 1).map_or(0, Vec::len);
                return (self.row - 1, prev_len);
            }
            return (self.row, 0);
        }
        // Skip whitespace immediately left of the cursor.
        while i > 0 && line.get(i - 1).is_some_and(|c| c.is_whitespace()) {
            i -= 1;
        }
        // Consume one class run (word chars OR punctuation chars).
        if let Some(&c) = line.get(i.wrapping_sub(1)) {
            let want_word = is_word_char(c);
            while i > 0 {
                match line.get(i - 1) {
                    Some(&c) if is_word_char(c) == want_word && !c.is_whitespace() => i -= 1,
                    _ => break,
                }
            }
        }
        (self.row, i)
    }

    /// The word-right target (mirror of [`word_left_target`](Self::word_left_target)).
    fn word_right_target(&self) -> (usize, usize) {
        let Some(line) = self.lines.get(self.row) else { return (self.row, self.col) };
        let len = line.len();
        let mut i = self.col;
        if i >= len {
            if self.row + 1 < self.lines.len() {
                return (self.row + 1, 0);
            }
            return (self.row, len);
        }
        while i < len && line.get(i).is_some_and(|c| c.is_whitespace()) {
            i += 1;
        }
        if let Some(&c) = line.get(i) {
            let want_word = is_word_char(c);
            while i < len {
                match line.get(i) {
                    Some(&c) if is_word_char(c) == want_word && !c.is_whitespace() => i += 1,
                    _ => break,
                }
            }
        }
        (self.row, i)
    }

    fn move_word_left(&mut self) {
        let (r, c) = self.word_left_target();
        self.row = r;
        self.col = c;
    }

    fn move_word_right(&mut self) {
        let (r, c) = self.word_right_target();
        self.row = r;
        self.col = c;
    }

    // ---- char-jump -------------------------------------------------------------------------

    /// Jump the cursor to the next/previous occurrence of `target` on the current line, skipping the
    /// current position (case-sensitive, `editor.ts:1990-2018`).
    fn jump_to(&mut self, dir: JumpDir, target: char) {
        let Some(line) = self.lines.get(self.row) else { return };
        match dir {
            JumpDir::Forward => {
                if let Some(off) = line.iter().enumerate().skip(self.col + 1).find_map(|(i, &c)| {
                    (c == target).then_some(i)
                }) {
                    self.col = off;
                }
            }
            JumpDir::Backward => {
                if let Some(off) = (0..self.col)
                    .rev()
                    .find(|&i| line.get(i) == Some(&target))
                {
                    self.col = off;
                }
            }
        }
    }

    // ---- history ---------------------------------------------------------------------------

    /// Add a raw submitted line to history (skip blank + consecutive-dup, `editor.ts:381-391`).
    fn add_to_history(&mut self, text: &str) {
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
    fn history_up_eligible(&self) -> bool {
        self.row == 0 && (self.is_empty() || self.history_index >= 0 || self.col == 0)
    }

    /// Older history entry (Up). On first entry, snapshot the draft.
    fn history_older(&mut self) {
        if self.history.is_empty() {
            return;
        }
        if self.history_index < 0 {
            self.history_draft = Some(self.snapshot());
        }
        let next = (self.history_index + 1).min(self.history.len() as isize - 1);
        self.history_index = next;
        if let Some(entry) = self.history.get(next as usize).cloned() {
            self.set_text(&entry);
            self.row = 0;
            self.col = 0; // cursor at start on Up (setTextInternal placement)
        }
    }

    /// Newer history entry (Down). Past the newest restores the saved draft.
    fn history_newer(&mut self) {
        if self.history_index < 0 {
            return;
        }
        self.history_index -= 1;
        if self.history_index < 0 {
            // Restore the draft.
            if let Some(draft) = self.history_draft.take() {
                self.lines = draft.lines;
                self.row = draft.row.min(self.lines.len().saturating_sub(1));
                self.col = draft.col.min(self.cur_len());
            } else {
                self.clear();
            }
        } else if let Some(entry) = self.history.get(self.history_index as usize).cloned() {
            self.set_text(&entry);
            self.move_end();
        }
    }

    /// Stop browsing history (any edit/newline/submit, `editor.ts`).
    fn exit_history(&mut self) {
        self.history_index = -1;
        self.history_draft = None;
    }

    // ---- autocomplete ----------------------------------------------------------------------

    /// The buffer lines as `String`s, for the autocomplete engine.
    fn lines_as_strings(&self) -> Vec<String> {
        self.lines.iter().map(|l| l.iter().collect()).collect()
    }

    /// Recompute the popup after an edit: auto-open only for slash context, otherwise update an
    /// already-open popup or close it (spec/tui/04 §5 — bare path does not auto-pop without Tab).
    fn update_autocomplete(&mut self) {
        let was_open = self.autocomplete.is_some();
        let computed = Autocomplete::compute(
            &self.registry,
            &self.lines_as_strings(),
            self.row,
            self.col,
            false,
            &self.cwd,
        );
        self.autocomplete = match computed {
            Some(ac) if ac.context == CompletionContext::Slash || was_open => Some(ac),
            _ => None,
        };
    }

    /// Trigger completion explicitly (Tab with no popup): force path completion, or slash completion
    /// while typing a `/name` (spec/tui/04 §5). A single forced match auto-applies (§3.7 item 10).
    fn trigger_completion(&mut self) -> EditorOutcome {
        let computed = Autocomplete::compute(
            &self.registry,
            &self.lines_as_strings(),
            self.row,
            self.col,
            true,
            &self.cwd,
        );
        match computed {
            Some(ac) if ac.list.len() == 1 && ac.context == CompletionContext::Path => {
                self.autocomplete = Some(ac);
                self.accept_completion();
                EditorOutcome::Edited
            }
            Some(ac) => {
                self.autocomplete = Some(ac);
                EditorOutcome::Edited
            }
            None => EditorOutcome::Ignored,
        }
    }

    /// Apply the selected popup item to the buffer (Tab/Enter accept). Leaves the popup state to the
    /// caller (Tab keeps editing + recomputes; Enter on a slash item submits).
    fn accept_completion(&mut self) {
        let Some(ac) = self.autocomplete.as_ref() else { return };
        if let Some(applied) = ac.apply(&self.lines_as_strings(), self.row, self.col) {
            self.lines = applied.lines.iter().map(|s| s.chars().collect()).collect();
            if self.lines.is_empty() {
                self.lines.push(Vec::new());
            }
            self.row = applied.cursor_line.min(self.lines.len().saturating_sub(1));
            self.col = applied.cursor_col.min(self.cur_len());
        }
    }

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
                self.push_undo_for(LastAction::Type);
                self.insert_char(c);
                self.last_action = LastAction::Type;
                self.exit_history();
                self.update_autocomplete();
                return EditorOutcome::Edited;
            }
        EditorOutcome::Ignored
    }

    /// Route a key while the popup is open. Returns `Some` if consumed; `None` to fall through.
    fn handle_popup_key(&mut self, ev: &KeyEvent) -> Option<EditorOutcome> {
        match ev.code {
            KeyCode::Esc => {
                self.autocomplete = None;
                Some(EditorOutcome::Edited)
            }
            KeyCode::Up => {
                if let Some(ac) = self.autocomplete.as_mut() {
                    ac.list.select_up();
                }
                Some(EditorOutcome::Edited)
            }
            KeyCode::Down => {
                if let Some(ac) = self.autocomplete.as_mut() {
                    ac.list.select_down();
                }
                Some(EditorOutcome::Edited)
            }
            KeyCode::Tab => {
                // Accept, keep editing (no submit), then recompute (may close if out of context).
                self.accept_completion();
                self.update_autocomplete();
                Some(EditorOutcome::Edited)
            }
            KeyCode::Enter => {
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
                    Some(EditorOutcome::Submit(text))
                } else {
                    self.update_autocomplete();
                    Some(EditorOutcome::Edited)
                }
            }
            _ => None,
        }
    }

    /// Dispatch a resolved editor action.
    fn apply_editor_action(&mut self, action: EditorAction) -> EditorOutcome {
        use EditorAction as E;
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
                if self.history_up_eligible() {
                    self.history_older();
                } else if self.row > 0 {
                    self.row -= 1;
                    self.col = self.col.min(self.cur_len());
                }
                EditorOutcome::Edited
            }
            E::CursorDown => {
                if self.history_index >= 0 {
                    self.history_newer();
                } else if self.row + 1 < self.lines.len() {
                    self.row += 1;
                    self.col = self.col.min(self.cur_len());
                }
                EditorOutcome::Edited
            }
            E::CursorWordLeft => {
                self.move_word_left();
                EditorOutcome::Edited
            }
            E::CursorWordRight => {
                self.move_word_right();
                EditorOutcome::Edited
            }
            E::CursorLineStart => {
                self.move_home();
                EditorOutcome::Edited
            }
            E::CursorLineEnd => {
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
                let text = self.text();
                if text.trim().is_empty() {
                    return EditorOutcome::Edited;
                }
                self.add_to_history(&text);
                self.clear();
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

    /// The hardware-cursor position inside `area` (for IME placement, R-10-009), if focused. Accounts
    /// for the top border rule drawn by [`Component::render`]; the editor has no side borders.
    pub fn cursor_in(&self, area: Rect) -> Option<(u16, u16)> {
        if !self.focused {
            return None;
        }
        let x = area.x.saturating_add(self.col.min(u16::MAX as usize) as u16);
        let y = area.y.saturating_add(1).saturating_add(self.row.min(u16::MAX as usize) as u16);
        let max_x = area.x.saturating_add(area.width).saturating_sub(1);
        let max_y = area.y.saturating_add(area.height).saturating_sub(1);
        Some((x.min(max_x), y.min(max_y)))
    }
}

/// Whether `c` is a word char (alphanumeric or `_`), for word-motion class runs.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl Component for InputEditor {
    /// Render the editor with **top + bottom rules only** (no side bars, no title) — Pi
    /// `editor.ts:476,517,575` (spec/tui/03 §3.1). The rule color flips to bash-green while the buffer
    /// starts with `!` (spec/tui/03 §7.1); otherwise it uses the border role, accented when focused.
    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        let rule_style = if self.is_bash_mode() {
            theme.bash_mode_style()
        } else if self.focused {
            theme.accent_style()
        } else {
            theme.border_style()
        };
        let block = Block::default()
            .borders(Borders::TOP | Borders::BOTTOM)
            .border_set(border::PLAIN)
            .border_style(rule_style);
        let lines: Vec<Line> = self
            .lines
            .iter()
            .map(|l| Line::styled(l.iter().collect::<String>(), theme.base_style()))
            .collect();
        let para = Paragraph::new(lines).block(block).style(theme.base_style());
        frame.render_widget(para, area);
        if let Some((x, y)) = self.cursor_in(area) {
            frame.set_cursor_position((x, y));
        }
    }
}
