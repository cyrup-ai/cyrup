//! The hand-rolled multi-line input editor (R-10-015; arch-10 §3.6, R-ARCH-TUI-013).
//!
//! No external editor crate (per task constraints): the buffer is a `Vec<Vec<char>>` (one inner vec
//! per line) so all cursor math is on `char` indices and never byte-indexes UTF-8 — important for
//! the no-panic policy (no `indexing_slicing`, R-00-009). Supports insert, newline, backspace,
//! forward-delete, arrow/Home/End movement, and Enter-to-submit. Wrapping/scrolling/paste-collapse
//! and stackable autocomplete are deferred (arch-10 §12) and noted below.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::component::Component;
use crate::theme::UiTheme;

/// Outcome of feeding a key to the editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorOutcome {
    /// Enter pressed: the submitted text (the buffer has been cleared).
    Submit(String),
    /// The buffer or cursor changed → request a re-render.
    Edited,
    /// The key was not handled by the editor.
    Ignored,
}

/// A multi-line text editor with a block cursor.
pub struct InputEditor {
    /// Lines as char vectors. Invariant: always at least one line.
    lines: Vec<Vec<char>>,
    /// Cursor row (`0..lines.len()`).
    row: usize,
    /// Cursor column in chars (`0..=lines[row].len()`).
    col: usize,
    /// Whether the editor currently owns focus (drives the hardware cursor for IME, R-10-009).
    focused: bool,
}

impl Default for InputEditor {
    fn default() -> Self {
        InputEditor::new()
    }
}

impl InputEditor {
    /// A fresh, empty, focused editor.
    pub fn new() -> Self {
        InputEditor { lines: vec![Vec::new()], row: 0, col: 0, focused: true }
    }

    /// Focus state (R-10-009 — focused inputs drive the hardware cursor).
    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }
    pub fn is_focused(&self) -> bool {
        self.focused
    }

    /// The full buffer text with `\n` line joins.
    pub fn text(&self) -> String {
        self.lines.iter().map(|l| l.iter().collect::<String>()).collect::<Vec<_>>().join("\n")
    }

    /// Whether the buffer is empty (no chars on any line).
    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(|l| l.is_empty())
    }

    /// Number of buffer lines (≥ 1) — used to size the editor area.
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// The cursor position as `(row, col)` in chars.
    pub fn cursor(&self) -> (usize, usize) {
        (self.row, self.col)
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

    /// Clear the buffer back to a single empty line.
    pub fn clear(&mut self) {
        self.lines = vec![Vec::new()];
        self.row = 0;
        self.col = 0;
    }

    /// The char length of the current line (0 if somehow out of range).
    fn cur_len(&self) -> usize {
        self.lines.get(self.row).map_or(0, Vec::len)
    }

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
        for c in s.chars() {
            if c == '\n' {
                self.insert_newline();
            } else {
                self.insert_char(c);
            }
        }
    }

    /// Split the current line at the cursor (Enter-as-newline).
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

    /// Backspace: delete the char before the cursor, joining lines at column 0.
    pub fn backspace(&mut self) {
        if self.col > 0 {
            let idx = self.col - 1;
            if let Some(line) = self.lines.get_mut(self.row)
                && idx < line.len()
            {
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
                && self.col < line.len()
            {
                line.remove(self.col);
            }
        } else if self.row + 1 < self.lines.len() {
            let next = self.lines.remove(self.row + 1);
            if let Some(line) = self.lines.get_mut(self.row) {
                line.extend(next);
            }
        }
    }

    /// Move the cursor one cell left (wrapping to the previous line's end).
    pub fn move_left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.cur_len();
        }
    }

    /// Move the cursor one cell right (wrapping to the next line's start).
    pub fn move_right(&mut self) {
        if self.col < self.cur_len() {
            self.col += 1;
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
    }

    /// Move up one line, clamping the column.
    pub fn move_up(&mut self) {
        if self.row > 0 {
            self.row -= 1;
            self.col = self.col.min(self.cur_len());
        }
    }

    /// Move down one line, clamping the column.
    pub fn move_down(&mut self) {
        if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = self.col.min(self.cur_len());
        }
    }

    /// Move to the start of the current line.
    pub fn move_home(&mut self) {
        self.col = 0;
    }

    /// Move to the end of the current line.
    pub fn move_end(&mut self) {
        self.col = self.cur_len();
    }

    /// Feed a key. Enter submits (returns the cleared text); Alt/Shift+Enter inserts a newline.
    /// Modifier+char chords (Ctrl/Super) are left for the global keymap and reported `Ignored`.
    pub fn handle_key(&mut self, ev: &KeyEvent) -> EditorOutcome {
        let alt = ev.modifiers.contains(KeyModifiers::ALT);
        let shift = ev.modifiers.contains(KeyModifiers::SHIFT);
        let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
        let supr = ev.modifiers.contains(KeyModifiers::SUPER);
        match ev.code {
            KeyCode::Enter if alt || shift => {
                self.insert_newline();
                EditorOutcome::Edited
            }
            KeyCode::Enter => {
                let text = self.text();
                self.clear();
                EditorOutcome::Submit(text)
            }
            KeyCode::Backspace => {
                self.backspace();
                EditorOutcome::Edited
            }
            KeyCode::Delete => {
                self.delete();
                EditorOutcome::Edited
            }
            KeyCode::Left => {
                self.move_left();
                EditorOutcome::Edited
            }
            KeyCode::Right => {
                self.move_right();
                EditorOutcome::Edited
            }
            KeyCode::Up => {
                self.move_up();
                EditorOutcome::Edited
            }
            KeyCode::Down => {
                self.move_down();
                EditorOutcome::Edited
            }
            KeyCode::Home => {
                self.move_home();
                EditorOutcome::Edited
            }
            KeyCode::End => {
                self.move_end();
                EditorOutcome::Edited
            }
            KeyCode::Char(c) if !ctrl && !supr => {
                self.insert_char(c);
                EditorOutcome::Edited
            }
            _ => EditorOutcome::Ignored,
        }
    }

    /// The hardware-cursor position inside `area` (for IME placement, R-10-009), if focused. Accounts
    /// for the one-cell border drawn by [`Component::render`].
    pub fn cursor_in(&self, area: Rect) -> Option<(u16, u16)> {
        if !self.focused {
            return None;
        }
        let x = area.x.saturating_add(1).saturating_add(self.col.min(u16::MAX as usize) as u16);
        let y = area.y.saturating_add(1).saturating_add(self.row.min(u16::MAX as usize) as u16);
        // Keep the cursor inside the bordered area.
        let max_x = area.x.saturating_add(area.width).saturating_sub(1);
        let max_y = area.y.saturating_add(area.height).saturating_sub(1);
        Some((x.min(max_x), y.min(max_y)))
    }
}

impl Component for InputEditor {
    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        let border_style = if self.focused { theme.accent_style() } else { theme.dim_style() };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(Span::styled(" message ", theme.dim_style()));
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
