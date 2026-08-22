use super::*;

impl InputEditor {
    // ---- motion ----------------------------------------------------------------------------

    pub fn move_left(&mut self) {
        if self.col > 0 {
            self.col = self.prev_grapheme(self.col);
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.cur_len();
        }
    }

    pub fn move_right(&mut self) {
        if self.col < self.cur_len() {
            self.col = self.next_grapheme(self.col);
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
    }

    /// The grapheme-cluster boundaries of the current line **with every valid paste marker merged
    /// into one cluster** — pi's `this.segment(text, "grapheme")` (`editor.ts:361-363`), the
    /// segmenter `moveCursor` (`:1808-1830`), `handleBackspace` (`:1287-1290`) and
    /// `handleForwardDelete` (`:1687-1690`) all step by. Without the merge the caret can be parked
    /// INSIDE a `[paste #N …]` marker, where the next keystroke silently destroys it (TUI-043's
    /// cursor-motion half).
    fn marker_grapheme_boundaries(&self, line: &[char]) -> Vec<usize> {
        let mut bounds = grapheme_boundaries(line);
        let markers = self.marker_spans(line);
        if !markers.is_empty() {
            bounds.retain(|&b| !markers.iter().any(|&(s, e, _)| b > s && b < e));
        }
        bounds
    }

    /// The previous grapheme-cluster boundary strictly left of char-column `col` on the current line
    /// (emoji/ZWJ/combining marks — and whole paste markers — step as one unit). `0` if none.
    pub(super) fn prev_grapheme(&self, col: usize) -> usize {
        let Some(line) = self.lines.get(self.row) else { return col.saturating_sub(1) };
        self.marker_grapheme_boundaries(line).into_iter().rfind(|&b| b < col).unwrap_or(0)
    }

    /// The next grapheme-cluster boundary strictly right of char-column `col` on the current line.
    /// Clamps to the line length when `col` is already at/after the last cluster.
    pub(super) fn next_grapheme(&self, col: usize) -> usize {
        let Some(line) = self.lines.get(self.row) else { return col + 1 };
        let len = line.len();
        self.marker_grapheme_boundaries(line).into_iter().find(|&b| b > col).unwrap_or(len)
    }

    pub fn move_home(&mut self) {
        self.col = 0;
    }

    pub fn move_end(&mut self) {
        self.col = self.cur_len();
    }

    /// Word-granularity segments of `text`, with every **valid** paste marker merged into one atomic
    /// segment — pi's `this.segment(text, "word")` (`editor.ts:361-363`), i.e. `segmentWithMarkers`
    /// (`:37-90`) over `Intl.Segmenter(undefined, { granularity: "word" })` (`utils.ts:5`).
    ///
    /// [CYRUP-DELTA] The base segmenter is `unicode_segmentation`'s UAX#29 word-boundary iterator
    /// rather than ICU's. They agree on Latin/Cyrillic/Greek prose, identifiers, `foo.bar`, `don't`
    /// and `3.14`; they differ on **unspaced scripts**, where ICU adds a dictionary/LSTM pass that
    /// UAX#29 alone has no data for — `你好世界` is two segments to ICU and four to UAX#29. Closing
    /// that needs an ICU-class word segmenter (`icu_segmenter` + its CJK/Thai data), which is a new
    /// workspace dependency and not this change's to take. See TUI-048.
    fn word_segments(&self, text: &[char]) -> Vec<WordSeg> {
        let markers = self.marker_spans(text);
        let joined: String = text.iter().collect();
        let mut out: Vec<WordSeg> = Vec::new();
        let mut col = 0usize;
        let mut mi = 0usize;
        for seg in joined.split_word_bounds() {
            let len = seg.chars().count();
            let start = col;
            col += len;
            // "Skip past markers that are entirely before this segment" (`editor.ts:67-69`).
            while markers.get(mi).is_some_and(|&(_, end, _)| end <= start) {
                mi += 1;
            }
            match markers.get(mi) {
                // "This segment falls inside a marker" (`:74`): emit the merged segment once, at the
                // marker's first base segment, and skip the rest (`:76-86`).
                Some(&(ms, me, _)) if start >= ms && start < me => {
                    if start == ms {
                        out.push(WordSeg {
                            start: ms,
                            len: me.saturating_sub(ms),
                            word_like: false,
                            atomic: true,
                        });
                    }
                }
                _ => out.push(WordSeg {
                    start,
                    len,
                    word_like: seg.chars().any(char::is_alphanumeric),
                    atomic: false,
                }),
            }
        }
        out
    }

    /// Whether `seg` is whitespace — pi's `isWhitespaceChar(segment)` = `/\s/.test(segment)`
    /// (`utils.ts:826-829`), which is *contains* whitespace, hence `any`.
    fn seg_is_whitespace(text: &[char], seg: &WordSeg) -> bool {
        text.get(seg.start..seg.start.saturating_add(seg.len))
            .is_some_and(|s| s.iter().any(|c| c.is_whitespace()))
    }

    /// The word-left target `(row, col)` — a statement-for-statement port of `findWordBackward`
    /// (`pi/packages/tui/src/word-navigation.ts:22-68` @v0.83.0) as pi calls it from
    /// `moveWordBackwards` (`editor.ts:1869-1889`), i.e. with
    /// `{ segment: (t) => this.segment(t, "word"), isAtomicSegment: isPasteMarker }`.
    ///
    /// Three branches after the whitespace skip: **one atomic segment** whole (`:44-46` — a
    /// `[paste #N …]` marker is never entered, which is what makes Ctrl+W delete the marker instead
    /// of chewing its closing `]`, TUI-043), **one word-like segment** truncated at its last
    /// `PUNCTUATION_REGEX` match (`:47-57`), or **a whole punctuation run** (`:58-66`).
    /// At col 0 step to the previous line's end (`editor.ts:1874-1881`).
    pub(super) fn word_left_target(&self) -> (usize, usize) {
        let Some(line) = self.lines.get(self.row) else { return (self.row, self.col) };
        let cursor = self.col.min(line.len());
        if cursor == 0 {
            if self.row > 0 {
                let prev_len = self.lines.get(self.row - 1).map_or(0, Vec::len);
                return (self.row - 1, prev_len);
            }
            return (self.row, 0);
        }
        // `const textBeforeCursor = text.slice(0, cursor)` (`:25`) — segmenting only the PREFIX is
        // why a marker the cursor sits inside is not atomic: it is not whole in this slice.
        let Some(before) = line.get(..cursor) else { return (self.row, cursor) };
        let mut segs = self.word_segments(before);
        let mut new_cursor = cursor;

        // "Skip trailing whitespace" (`:31-38`).
        while let Some(last) = segs.last() {
            if last.atomic || !Self::seg_is_whitespace(before, last) {
                break;
            }
            new_cursor = new_cursor.saturating_sub(last.len);
            segs.pop();
        }
        // `if (segments.length === 0) return newCursor` (`:40`).
        let Some(&last) = segs.last() else { return (self.row, new_cursor) };

        if last.atomic {
            // "Skip one atomic segment" (`:44-46`).
            new_cursor = new_cursor.saturating_sub(last.len);
        } else if last.word_like {
            // "Skip inside one word-like segment, preserving ASCII punctuation boundaries"
            // (`:47-57`): back up to just after the LAST punctuation character in the segment.
            let seg = before.get(last.start..last.start.saturating_add(last.len)).unwrap_or(&[]);
            match seg.iter().rposition(|&c| is_punctuation(c)) {
                None => new_cursor = new_cursor.saturating_sub(last.len),
                Some(idx) => {
                    new_cursor = new_cursor.saturating_sub(last.len.saturating_sub(idx + 1));
                }
            }
        } else {
            // "Skip non-word non-whitespace run (punctuation)" (`:58-66`).
            while let Some(last) = segs.last() {
                if last.atomic || last.word_like || Self::seg_is_whitespace(before, last) {
                    break;
                }
                new_cursor = new_cursor.saturating_sub(last.len);
                segs.pop();
            }
        }
        (self.row, new_cursor)
    }

    /// The word-right target — the mirror port of `findWordForward` (`word-navigation.ts:76-114`),
    /// called as `moveWordForwards` does (`editor.ts:2064-2083`). Same three branches, with the
    /// atomic skip at `:97-99` and the word-like branch taking the FIRST punctuation match (`:102`).
    pub(super) fn word_right_target(&self) -> (usize, usize) {
        let Some(line) = self.lines.get(self.row) else { return (self.row, self.col) };
        let len = line.len();
        let cursor = self.col.min(len);
        if cursor >= len {
            if self.row + 1 < self.lines.len() {
                return (self.row + 1, 0);
            }
            return (self.row, len);
        }
        // `const textAfterCursor = text.slice(cursor)` (`:79`).
        let Some(after) = line.get(cursor..) else { return (self.row, cursor) };
        let segs = self.word_segments(after);
        let mut idx = 0usize;
        let mut new_cursor = cursor;

        // "Skip leading whitespace" (`:88-93`).
        while let Some(seg) = segs.get(idx) {
            if seg.atomic || !Self::seg_is_whitespace(after, seg) {
                break;
            }
            new_cursor = new_cursor.saturating_add(seg.len);
            idx += 1;
        }
        // `if (next.done) return newCursor` (`:95`).
        let Some(&next) = segs.get(idx) else { return (self.row, new_cursor) };

        if next.atomic {
            new_cursor = new_cursor.saturating_add(next.len);
        } else if next.word_like {
            let seg = after.get(next.start..next.start.saturating_add(next.len)).unwrap_or(&[]);
            let step = seg.iter().position(|&c| is_punctuation(c)).unwrap_or(next.len);
            new_cursor = new_cursor.saturating_add(step);
        } else {
            while let Some(seg) = segs.get(idx) {
                if seg.atomic || seg.word_like || Self::seg_is_whitespace(after, seg) {
                    break;
                }
                new_cursor = new_cursor.saturating_add(seg.len);
                idx += 1;
            }
        }
        (self.row, new_cursor)
    }

    pub(super) fn move_word_left(&mut self) {
        let (r, c) = self.word_left_target();
        self.row = r;
        self.col = c;
    }

    pub(super) fn move_word_right(&mut self) {
        let (r, c) = self.word_right_target();
        self.row = r;
        self.col = c;
    }

    // ---- char-jump -------------------------------------------------------------------------

    /// Jump the cursor to the next/previous occurrence of `target` on the current line, skipping the
    /// current position (case-sensitive, `editor.ts:1990-2018`).
    pub(super) fn jump_to(&mut self, dir: JumpDir, target: char) {
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
}

/// The ASCII punctuation that sub-divides a word-like segment — a literal port of
/// `PUNCTUATION_REGEX` (`pi/packages/tui/src/utils.ts:821` @v0.83.0):
///
/// ```text
/// /[(){}[\]<>.,;:'"!?+\-=*/\\|&%^$#@~`]/
/// ```
///
/// Deliberately **not** the complement of an `is_alphanumeric() || '_'` word-char test — the two are
/// different sets (that test rejects every non-alphanumeric; this one names 31 specific ASCII
/// characters), and word navigation must use pi's. The old class-run word motion used the former and
/// was replaced wholesale (TUI-043 / TUI-048).
fn is_punctuation(c: char) -> bool {
    matches!(
        c,
        '(' | ')'
            | '{'
            | '}'
            | '['
            | ']'
            | '<'
            | '>'
            | '.'
            | ','
            | ';'
            | ':'
            | '\''
            | '"'
            | '!'
            | '?'
            | '+'
            | '-'
            | '='
            | '*'
            | '/'
            | '\\'
            | '|'
            | '&'
            | '%'
            | '^'
            | '$'
            | '#'
            | '@'
            | '~'
            | '`'
    )
}
