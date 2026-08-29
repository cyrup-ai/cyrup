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
    ///
    /// Also the segmenter VERTICAL motion snaps against —
    /// [`move_to_visual_line`](Self::move_to_visual_line) re-segments the TARGET logical line with
    /// it, which is why this is `pub(super)` rather than private to this module
    /// (`moveToVisualLine`, `editor.ts:1428`).
    pub(super) fn marker_grapheme_boundaries(&self, line: &[char]) -> Vec<usize> {
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

    /// The word-left target `(row, col)` — pi's `findWordBackward`
    /// (`pi/packages/tui/src/word-navigation.ts:22-68` @v0.83.0) as `moveWordBackwards` calls it
    /// (`editor.ts:1869-1889`), i.e. with
    /// `{ segment: (t) => this.segment(t, "word"), isAtomicSegment: isPasteMarker }`.
    ///
    /// The segment walk itself lives in [`find_word_backward`],
    /// shared with the single-line [`crate::text_input::Input`]; what stays here is what has no
    /// single-line analogue — the marker-aware char-column segmentation and the row edge, where col
    /// 0 steps to the previous line's end (`editor.ts:1874-1881`).
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
        let segs = self.word_segments(before);
        let new_cursor = find_word_backward(
            segs,
            cursor,
            &|seg| Self::seg_is_whitespace(before, seg),
            &|seg| {
                let s = before.get(seg.start..seg.start.saturating_add(seg.len)).unwrap_or(&[]);
                s.iter().rposition(|&c| is_punctuation(c)).map(|i| i.saturating_add(1))
            },
        );
        (self.row, new_cursor)
    }

    /// The word-right target — the mirror of [`Self::word_left_target`], pi's `findWordForward`
    /// (`word-navigation.ts:76-114`) as `moveWordForwards` calls it (`editor.ts:2064-2083`). The
    /// walk is [`find_word_forward`]; the row edge (col ==
    /// line length steps to the next line's start) stays here.
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
        let new_cursor = find_word_forward(
            &segs,
            cursor,
            &|seg| Self::seg_is_whitespace(after, seg),
            &|seg| {
                let s = after.get(seg.start..seg.start.saturating_add(seg.len)).unwrap_or(&[]);
                s.iter().position(|&c| is_punctuation(c))
            },
        );
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

    /// Jump the cursor to the next/previous occurrence of `target`. **Multi-line** search,
    /// case-sensitive, skipping the current cursor position — `jumpToChar`
    /// (`editor.ts:2043-2074` @v0.84.3; the previous cite `editor.ts:1990-2018` was version drift).
    ///
    /// The scan starts on the caret's own line and walks outward to the buffer edge (`:2051-2054`):
    /// on that first line it begins one column past/before the caret, on every later line it covers
    /// the whole line (`:2056-2063`). A hit moves **both** `row` and `col` (`:2067-2069`); no hit
    /// anywhere leaves the cursor exactly where it was (`:2073`). The old port only ever searched
    /// the current line and only ever assigned `col`.
    pub(super) fn jump_to(&mut self, dir: JumpDir, target: char) {
        self.last_action = LastAction::None; // `this.lastAction = null` (`editor.ts:2047`)
        let hit = match dir {
            JumpDir::Forward => (self.row..self.lines.len()).find_map(|idx| {
                let line = self.lines.get(idx)?;
                // `searchFrom = isCurrentLine ? cursorCol + 1 : undefined` (`:2056-2063`) fed to
                // `String.indexOf` (`:2065`), i.e. an inclusive lower bound.
                let from = if idx == self.row { self.col.saturating_add(1) } else { 0 };
                let rest = line.get(from..)?;
                rest.iter().position(|&c| c == target).map(|off| (idx, from.saturating_add(off)))
            }),
            JumpDir::Backward => (0..=self.row).rev().find_map(|idx| {
                let line = self.lines.get(idx)?;
                // `searchFrom = cursorCol - 1` (`:2060`) is an INCLUSIVE upper bound for
                // `String.lastIndexOf` (`:2065`).
                //
                // Upstream quirk, reproduced deliberately: at `cursorCol === 0` that expression is
                // `-1`, and `lastIndexOf(char, -1)` clamps its start to 0, so index 0 is still
                // examined — a caret already sitting on a matching char at column 0 "jumps" to
                // itself instead of continuing to earlier lines. `saturating_sub(1)` yields the
                // same `0..=0` window. It is observable behaviour of the upstream this port is
                // measured against, so it is mirrored rather than quietly corrected.
                let upper = if idx == self.row {
                    self.col.saturating_sub(1)
                } else {
                    line.len().checked_sub(1)?
                };
                let scanned = line.get(..=upper)?;
                scanned.iter().rposition(|&c| c == target).map(|off| (idx, off))
            }),
        };
        if let Some((row, col)) = hit {
            self.row = row;
            // `setCursorCol` (`:2069` → `:1377-1381`) also drops the sticky vertical-motion state.
            // The jump path runs in `handle_key` ahead of `apply_editor_action`, so it never reaches
            // that crate's central `reset_preferred_col` gate (`keys.rs`) and has to do it here —
            // otherwise the Up/Down after a jump steers by a stale goal column.
            self.col = col;
            self.reset_preferred_col();
        }
    }
}
