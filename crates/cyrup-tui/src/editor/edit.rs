use super::*;

impl InputEditor {
    // ---- insertion -------------------------------------------------------------------------

    /// Insert a printable character at the cursor.
    pub fn insert_char(&mut self, c: char) {
        let col = self.col.min(self.cur_len());
        if let Some(line) = self.lines.get_mut(self.row) {
            line.insert(col, c);
            self.col = col + 1;
        }
        self.preferred_visual_col = None;
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

    /// Backspace: delete the whole grapheme cluster before the cursor (emoji/ZWJ/combining marks
    /// removed as one unit, `editor.ts`), joining lines at column 0.
    pub fn backspace(&mut self) {
        if self.col > 0 {
            // The cluster about to be deleted, marker-aware ([`marker_grapheme_boundaries`]) — pi
            // takes the LAST segment of `line.slice(0, cursorCol)` under `this.segment(…,
            // "grapheme")` (`editor.ts:1287-1290`), which is a whole `[paste #N …]` marker exactly
            // when the caret sits on the marker's closing `]`.
            let start = self.prev_grapheme(self.col);
            // "This contains the id part e.g 4 from [paste #4 +123 lines]" (`editor.ts:1291-1315`):
            // when the deleted cluster IS a marker, drop its registry entry and renumber.
            let deleted_marker = self
                .lines
                .get(self.row)
                .and_then(|line| self.marker_at(line, start))
                .filter(|&(_, _, end)| end == self.col)
                .map(|(id, _, _)| id);
            if let Some(target) = deleted_marker {
                self.drop_paste(target);
            }
            if let Some(line) = self.lines.get_mut(self.row) {
                let end = self.col.min(line.len());
                if start < end {
                    line.drain(start..end);
                }
            }
            self.col = start;
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

    /// Forward-delete: delete the whole grapheme cluster at the cursor (one user-perceived char),
    /// joining the next line at end-of-line.
    ///
    /// The cluster is marker-aware ([`marker_grapheme_boundaries`](Self::marker_grapheme_boundaries)),
    /// so Delete at a marker's `[` removes the whole marker — pi's `handleForwardDelete` takes the
    /// FIRST segment of `line.slice(cursorCol)` under `this.segment(…, "grapheme")`
    /// (`editor.ts:1687-1690`). Note the deliberate asymmetry with [`backspace`](Self::backspace):
    /// upstream's forward-delete has **no** paste branch — it neither drops the registry entry nor
    /// renumbers (`:1674-1706`), so neither does this.
    pub fn delete(&mut self) {
        let len = self.cur_len();
        if self.col < len {
            let end = self.next_grapheme(self.col);
            if let Some(line) = self.lines.get_mut(self.row) {
                let end = end.min(line.len());
                if self.col < end {
                    line.drain(self.col..end);
                }
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
    pub(super) fn delete_word_backward(&mut self) {
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
    pub(super) fn delete_word_forward(&mut self) {
        let target = self.word_right_target();
        if target == (self.row, self.col) {
            return;
        }
        let killed = self.take_range((self.row, self.col), target);
        self.push_kill(&killed, true);
    }

    /// Delete from line start to the cursor, feeding the kill ring (Ctrl+U).
    pub(super) fn delete_to_line_start(&mut self) {
        if self.col == 0 {
            return;
        }
        let killed = self.take_range((self.row, 0), (self.row, self.col));
        self.push_kill(&killed, false);
        self.col = 0;
    }

    /// Delete from the cursor to line end, feeding the kill ring (Ctrl+K).
    pub(super) fn delete_to_line_end(&mut self) {
        let len = self.cur_len();
        if self.col >= len {
            return;
        }
        let killed = self.take_range((self.row, self.col), (self.row, len));
        self.push_kill(&killed, true);
    }

    /// Remove and return the text between two positions, **crossing logical lines** (item #5: cross-
    /// line word/char ops). A same-line range drains within the row; a multi-line range removes the
    /// tail of `start.0`, every whole line strictly between, and the head of `end.0`, then joins the
    /// two boundary rows — so a `Ctrl+W`/`Alt+D` (or `Backspace`/`Delete`) at a line edge deletes into
    /// the neighbouring line and re-joins it (`editor.ts` word/char deletion; `word-navigation.ts`
    /// returns cross-line targets). The removed text carries the `\n`s so it yanks back verbatim.
    fn take_range(&mut self, start: (usize, usize), end: (usize, usize)) -> String {
        // Normalize so `start <= end`.
        let (start, end) =
            if (start.0, start.1) <= (end.0, end.1) { (start, end) } else { (end, start) };
        if start == end {
            return String::new();
        }
        // Same-line: drain within the row.
        if start.0 == end.0 {
            let Some(line) = self.lines.get_mut(start.0) else { return String::new() };
            let lo = start.1.min(line.len());
            let hi = end.1.min(line.len());
            if lo >= hi {
                return String::new();
            }
            return line.drain(lo..hi).collect();
        }
        // Multi-line: guard the boundary rows.
        if start.0 >= self.lines.len() || end.0 >= self.lines.len() {
            return String::new();
        }
        let start_col = start.1.min(self.lines.get(start.0).map_or(0, Vec::len));
        let end_col = end.1.min(self.lines.get(end.0).map_or(0, Vec::len));

        // Collect the removed text (start tail + whole inner rows + end head), `\n`-joined, so it
        // yanks back verbatim.
        let mut killed = String::new();
        if let Some(first) = self.lines.get(start.0) {
            killed.extend(first.iter().skip(start_col));
        }
        for r in (start.0 + 1)..end.0 {
            killed.push('\n');
            if let Some(row) = self.lines.get(r) {
                killed.extend(row.iter());
            }
        }
        killed.push('\n');
        if let Some(last) = self.lines.get(end.0) {
            killed.extend(last.iter().take(end_col));
        }

        // Splice: keep the head of `start.0`, append the tail of `end.0`, drop the rows between.
        let tail: Vec<char> = self
            .lines
            .get(end.0)
            .map(|l| l.iter().skip(end_col).copied().collect())
            .unwrap_or_default();
        if let Some(first) = self.lines.get_mut(start.0) {
            first.truncate(start_col);
            first.extend(tail);
        }
        self.lines.drain((start.0 + 1)..=end.0);
        killed
    }
}
