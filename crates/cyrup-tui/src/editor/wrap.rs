use super::*;

impl InputEditor {
    // ---- visual-line map (wrap-aware vertical motion, spec/tui/03 §4) -----------------------

    /// Record the layout width used to wrap lines for vertical motion (set every render; also a test
    /// seam). `0` is clamped to `1` so wrapping never divides by zero.
    pub fn set_view_width(&mut self, width: usize) {
        self.view_width = width.max(1);
    }

    /// Tell the editor how many rows the host TERMINAL has — pi's `this.tui.terminal.rows`
    /// (`editor.ts:500`). Drives [`term_rows`](Self::term_rows), i.e. the `max(5, floor(rows *
    /// 0.3))` window the editor caps itself at. The app publishes this every `draw`; an embedder
    /// that never calls it gets pi's `?? 24` default.
    pub fn set_terminal_height(&mut self, rows: u16) {
        self.term_rows = rows.max(1);
    }

    /// The editor's own visible-row budget at the current [`term_rows`](Self::term_rows) —
    /// `Math.max(5, Math.floor(terminalRows * 0.3))` (`editor.ts:501`).
    pub fn max_visible_lines(&self) -> u16 {
        crate::app::max_visible_editor_lines(self.term_rows)
    }

    /// Build the wrap-aware visual-line map at the current [`view_width`](Self::view_width)
    /// (`editor.ts:1690` `build_visual_line_map`). Each logical line expands into one or more
    /// [`VisualLine`]s via word-aware wrapping; the result is in reading order and always non-empty
    /// (at least one zero-length visual line for the single empty buffer line).
    pub fn visual_line_map(&self) -> Vec<VisualLine> {
        let width = self.view_width.max(1);
        let mut map = Vec::with_capacity(self.lines.len());
        for (logical, line) in self.lines.iter().enumerate() {
            for (start, len) in word_wrap_line(line, width) {
                map.push(VisualLine {
                    logical,
                    start,
                    len,
                });
            }
        }
        if map.is_empty() {
            map.push(VisualLine {
                logical: 0,
                start: 0,
                len: 0,
            });
        }
        map
    }

    /// The number of **visual** (wrapped) lines the buffer occupies at `width` columns — the total
    /// [`VisualLine`] count of the wrap map built at an arbitrary width (`editor.ts:1690`
    /// `build_visual_line_map(width).length`, the same primitive vertical motion uses). The app sizes
    /// the editor slot from this so a long/pasted logical line grows the box one row per wrapped
    /// visual line instead of clipping. Independent of [`view_width`](Self::view_width) so height
    /// measurement and render agree when passed the same width. Always `>= 1`.
    pub fn visual_line_count(&self, width: usize) -> usize {
        let width = width.max(1);
        let count: usize = self
            .lines
            .iter()
            .map(|line| word_wrap_line(line, width).len())
            .sum();
        count.max(1)
    }

    /// Map the cursor `(row, col)` to its index in `map` (`editor.ts:1742` `find_current_visual_line`):
    /// the visual line of the cursor's logical row that contains `col`; when `col` sits exactly on a
    /// wrap boundary it belongs to the *start* of the following visual line, and an end-of-line cursor
    /// rides the last visual line of the row.
    pub(super) fn current_visual_line(&self, map: &[VisualLine]) -> usize {
        find_visual_line_at(map, self.row, self.col)
    }

    /// Move the caret to `target` in `map`, applying the sticky-column table, the non-last-segment
    /// clamp and the atomic-segment snap — a port of `moveToVisualLine` (`editor.ts:1383-1466`),
    /// documented upstream as "Shared by moveCursor() and pageScroll()". All three of cyrup's
    /// vertical movers ([`move_up_visual`](Self::move_up_visual),
    /// [`move_down_visual`](Self::move_down_visual), [`page_scroll`](Self::page_scroll)) go through
    /// it, exactly as upstream's two entry points do.
    ///
    /// Four things happen here, in upstream's order:
    ///
    /// 1. **Pre-snap column re-resolution** (`:1396-1404`). When [`snapped_from_col`] holds a
    ///    stash, the SOURCE visual column comes from that pre-snap column resolved through
    ///    [`find_visual_line_at`] rather than from the live (snapped) `col` — "this gives the
    ///    correct visual column even after a resize reshuffles VLs".
    /// 2. **The non-last-segment clamp** (`:1406-1416`), see [`is_last_segment`].
    /// 3. **The sticky-column table** (`:1418`), see
    ///    [`compute_vertical_move_column`](Self::compute_vertical_move_column).
    /// 4. **The atomic-segment snap** (`:1425-1466`): the TARGET logical line is re-segmented with
    ///    [`marker_grapheme_boundaries`](Self::marker_grapheme_boundaries) — pi's
    ///    `this.segment(logicalLine, "grapheme")` — and a landing inside a multi-grapheme segment
    ///    either skips the segment's remaining continuation rows (moving down into a segment that
    ///    began on an earlier visual row, `:1437-1453`) or snaps back to the segment start, stashing
    ///    the pre-snap column (`:1455-1461`). Landing outside every atomic segment clears the stash
    ///    (`:1465`). This is what stops Up/Down/PageUp/PageDown parking the caret INSIDE a
    ///    `[paste #N …]` marker on a wrapped line, where the next Backspace would shred it (the
    ///    `end == self.col` filter in [`backspace`](Self::backspace) can only fire on a whole
    ///    marker).
    ///
    /// [`snapped_from_col`]: Self::snapped_from_col
    ///
    /// [CYRUP-DELTA] Upstream re-enters `moveToVisualLine` recursively for the continuation skip;
    /// this is the same walk written as a loop, with `next > target` made an explicit guard rather
    /// than the monotonicity upstream relies on implicitly, so a pathological map cannot spin.
    pub(super) fn move_to_visual_line(
        &mut self,
        map: &[VisualLine],
        current: usize,
        target: usize,
    ) {
        let mut target = target;
        loop {
            // `if (!(currentVL && targetVL)) return` (`:1391-1394`).
            let (Some(current_vl), Some(target_vl)) =
                (map.get(current).copied(), map.get(target).copied())
            else {
                return;
            };

            // "When the cursor was snapped to a segment start, resolve the pre-snap position
            // against the VL it belongs to." (`:1396-1404`).
            let current_visual_col = match self.snapped_from_col {
                Some(pre_snap) => {
                    let i = find_visual_line_at(map, current_vl.logical, pre_snap);
                    pre_snap.saturating_sub(map.get(i).map_or(0, |vl| vl.start))
                }
                None => self.col.saturating_sub(current_vl.start),
            };

            // "For non-last segments, clamp to length-1 to stay within the segment" (`:1406-1416`).
            let source_max = segment_max_visual_col(map, current);
            let target_max = segment_max_visual_col(map, target);
            let move_col =
                self.compute_vertical_move_column(current_visual_col, source_max, target_max);

            // "Set cursor position" (`:1420-1423`).
            self.row = target_vl.logical;
            let line_len = self.cur_len();
            self.col = target_vl.start.saturating_add(move_col).min(line_len);

            // "Snap cursor to atomic segment boundary (e.g. paste markers) so the cursor never lands
            // in the middle of a multi-grapheme unit." (`:1425-1463`). Upstream iterates the
            // segmenter's segments; cyrup's segmenter yields BOUNDARIES, so a segment is a
            // consecutive boundary pair.
            let bounds = match self.lines.get(self.row) {
                Some(line) => self.marker_grapheme_boundaries(line),
                None => Vec::new(),
            };
            let mut next_target: Option<usize> = None;
            let mut snapped = false;
            for (&seg_start, &seg_end) in bounds.iter().zip(bounds.iter().skip(1)) {
                // `if (seg.index > this.state.cursorCol) break` (`:1430`).
                if seg_start > self.col {
                    break;
                }
                // `if (seg.segment.length <= 1) continue` (`:1431`). [CYRUP-DELTA] upstream's
                // `.length` is UTF-16 code units against a code-unit cursor; cyrup's columns are
                // CHAR indices, so the analogous "single unit the caret cannot land inside" test is
                // the segment's char count.
                if seg_end.saturating_sub(seg_start) <= 1 {
                    continue;
                }
                // `if (this.state.cursorCol < seg.index + seg.segment.length)` (`:1432`).
                if self.col >= seg_end {
                    continue;
                }
                let is_continuation = seg_start < target_vl.start;
                let is_moving_down = target > current;
                if is_continuation && is_moving_down {
                    // "The segment started on a previous visual line, and we already visited it on
                    // the way down. Skip all remaining continuation VLs and land on the first VL
                    // past it." (`:1436-1453`).
                    let mut next = target.saturating_add(1);
                    while map
                        .get(next)
                        .is_some_and(|vl| vl.logical == target_vl.logical && vl.start < seg_end)
                    {
                        next = next.saturating_add(1);
                    }
                    if next < map.len() {
                        next_target = Some(next);
                        break;
                    }
                }
                // "Snap to the start of the segment so it gets highlighted. Store the pre-snap
                // position so the next vertical move can resolve it to the correct visual column."
                // (`:1455-1461`).
                self.snapped_from_col = Some(self.col);
                self.col = seg_start;
                snapped = true;
                break;
            }

            match next_target {
                // The re-entry (`:1450`), as a loop; `next` is seeded at `target + 1` so the guard
                // always holds and the walk always advances.
                Some(next) if next > target => target = next,
                _ => {
                    if !snapped {
                        // "No snap occurred – we moved out of the atomic segment." (`:1465`).
                        self.snapped_from_col = None;
                    }
                    return;
                }
            }
        }
    }

    /// The target visual column for a vertical move — a statement-for-statement port of
    /// `computeVerticalMoveColumn` (`editor.ts:1489-1518`) and the sticky-column decision table it
    /// documents at `:1470-1488`:
    ///
    /// ```text
    /// | P | S | T | U | Scenario                                             | Set Preferred | Move To     |
    /// |---|---|---|---| ---------------------------------------------------- |---------------|-------------|
    /// | 0 | * | 0 | - | Start nav, target fits                               | null          | current     |
    /// | 0 | * | 1 | - | Start nav, target shorter                            | current       | target end  |
    /// | 1 | 0 | 0 | 0 | Clamped, target fits preferred                       | null          | preferred   |
    /// | 1 | 0 | 0 | 1 | Clamped, target longer but still can't fit preferred | keep          | target end  |
    /// | 1 | 0 | 1 | - | Clamped, target even shorter                         | keep          | target end  |
    /// | 1 | 1 | 0 | - | Rewrapped, target fits current                       | null          | current     |
    /// | 1 | 1 | 1 | - | Rewrapped, target shorter than current               | current       | target end  |
    ///
    /// Where:
    /// - P = preferred col is set
    /// - S = cursor in middle of source line (not clamped to end)
    /// - T = target line shorter than current visual col
    /// - U = target line shorter than preferred col
    /// ```
    ///
    /// Note that three of the seven cases CLEAR [`preferred_visual_col`](Self::preferred_visual_col)
    /// (1, 3 and 6) and only two set it (2 and 7) — the previous cyrup movers set it
    /// unconditionally on every vertical step, which is case 3's "consume the preference" arm lost.
    fn compute_vertical_move_column(
        &mut self,
        current_visual_col: usize,
        source_max_visual_col: usize,
        target_max_visual_col: usize,
    ) -> usize {
        let cursor_in_middle = current_visual_col < source_max_visual_col; // S
        let target_too_short = target_max_visual_col < current_visual_col; // T

        // `if (!hasPreferred || cursorInMiddle)` (`:1499`) — P is the `is_some`, so binding the
        // preferred value here folds the two guards into one refutable pattern and keeps the
        // `unwrap` upstream needs at `:1512` out of the port entirely.
        let Some(preferred) = self.preferred_visual_col.filter(|_| !cursor_in_middle) else {
            if target_too_short {
                // Cases 2 and 7 (`:1500-1503`).
                self.preferred_visual_col = Some(current_visual_col);
                return target_max_visual_col;
            }
            // Cases 1 and 6 (`:1505-1506`).
            self.preferred_visual_col = None;
            return current_visual_col;
        };

        let target_cant_fit_preferred = target_max_visual_col < preferred; // U
        if target_too_short || target_cant_fit_preferred {
            // Cases 4 and 5 (`:1510-1513`) — the preference is KEPT.
            return target_max_visual_col;
        }

        // Case 3 (`:1515-1517`).
        self.preferred_visual_col = None;
        preferred
    }

    /// Vertical Up by one **visual** line, preserving the sticky preferred column (spec/tui/03 §4.2).
    /// At the first visual line the cursor falls through to line-start (history is handled by the
    /// caller before this runs).
    pub(super) fn move_up_visual(&mut self) {
        let map = self.visual_line_map();
        let cur = self.current_visual_line(&map);
        if cur == 0 {
            // First visual line: fall through to line-start (spec/tui/03 §5.1). This is cyrup's
            // deliberate divergence from `moveCursor`, which simply does not move when the target
            // index is out of range (`editor.ts:1808-1812`) — but the placement it performs IS
            // `moveToLineStart`, and upstream's `moveToLineStart` is `setCursorCol(0)`
            // (`:1522-1525`), which drops BOTH sticky-column fields. Anything else would leave a
            // stash from a snapped caret to misresolve the next vertical move.
            self.reset_preferred_col();
            self.col = 0;
            return;
        }
        self.move_to_visual_line(&map, cur, cur.saturating_sub(1));
    }

    /// Vertical Down by one **visual** line, preserving the sticky preferred column (spec/tui/03 §4.2).
    /// At the last visual line the cursor falls through to line-end (history is handled by the caller).
    pub(super) fn move_down_visual(&mut self) {
        let map = self.visual_line_map();
        let cur = self.current_visual_line(&map);
        if cur + 1 >= map.len() {
            // Last visual line: fall through to line-end (spec/tui/03 §5.1) — the mirror of
            // [`move_up_visual`](Self::move_up_visual)'s fall-through, and upstream's
            // `moveToLineEnd` is likewise `setCursorCol(line.length)` (`editor.ts:1527-1531`).
            self.reset_preferred_col();
            self.col = self.cur_len();
            return;
        }
        self.move_to_visual_line(&map, cur, cur.saturating_add(1));
    }

    /// Move the caret by one **page** of visual lines (`editor.ts:1857` `pageScroll(direction)`;
    /// `direction` is `-1` for up, `1` for down).
    ///
    /// Upstream:
    ///
    /// ```text
    /// const pageSize = Math.max(5, Math.floor(terminalRows * 0.3));
    /// const target = Math.max(0, Math.min(visualLines.length - 1, current + direction * pageSize));
    /// this.moveToVisualLine(visualLines, current, target);
    /// ```
    ///
    /// The page size is the SAME `max(5, floor(rows * 0.3))` window the editor renders in
    /// ([`max_visible_lines`](Self::max_visible_lines)), and the move goes through the shared sticky
    /// preferred-column machinery, so a page hop keeps the goal column exactly as Up/Down do.
    ///
    /// Unlike [`move_up_visual`](Self::move_up_visual) / [`move_down_visual`](Self::move_down_visual)
    /// there is **no** history recall and no line-start/line-end fall-through at the ends: upstream
    /// clamps the target index and lets `moveToVisualLine` place the caret (`editor.ts:1863`).
    pub fn page_scroll(&mut self, direction: i8) {
        self.last_action = LastAction::None;
        let page = usize::from(self.max_visible_lines());
        let map = self.visual_line_map();
        let cur = self.current_visual_line(&map);
        let last = map.len().saturating_sub(1);
        let target = if direction < 0 {
            cur.saturating_sub(page)
        } else {
            (cur + page).min(last)
        };
        self.move_to_visual_line(&map, cur, target);
    }

    /// Whether the buffer occupies more than one **visual** line at the current layout width, i.e.
    /// whether there is anything inside the editor for a page hop to move through.
    ///
    /// The app consults this to decide whether `PageUp`/`PageDown` belongs to the editor (pi's only
    /// binding for those keys, `keybindings.ts:89-90`) or falls through to cyrup's active-region
    /// transcript scroll — see [`crate::app::App::handle_input`].
    pub fn is_multi_visual_line(&self) -> bool {
        self.visual_line_map().len() > 1
    }

    /// Drop **both** sticky vertical-motion fields — the preferred column and the pre-snap stash —
    /// so the next Up/Down re-seeds the goal column from the live cursor. cyrup's `setCursorCol`
    /// (`editor.ts:1377-1381`), which is what upstream uses "for all non-vertical cursor movements
    /// to reset sticky column behavior"; every caller here is likewise a non-vertical
    /// motion/edit/paste/undo, and the central gate in `apply_editor_action` (`keys.rs`) exempts
    /// exactly `CursorUp`/`CursorDown`/`PageUp`/`PageDown`.
    pub(super) fn reset_preferred_col(&mut self) {
        self.preferred_visual_col = None;
        self.snapped_from_col = None;
    }
}

/// The visual-line index containing logical position `(line, col)` — pi's `findVisualLineAt`
/// (`editor.ts:1774-1792`). A `col` exactly on a wrap boundary belongs to the *start* of the
/// following visual line; on the LAST visual line of a logical line an end-of-line `col` (`start +
/// len`) still rides that line, which is what upstream spells as its `isLastSegmentOfLine &&
/// offset === vl.length` arm.
///
/// [CYRUP-DELTA] Upstream's miss fallback is `visualLines.length - 1`, the last visual line of the
/// whole BUFFER. This returns the last visual line of the requested logical `line` instead (`0` when
/// the map has none), which is the only answer a caller can use: both call sites — the cursor's own
/// position and the pre-snap stash in [`InputEditor::move_to_visual_line`] — pass a `line` that is
/// in the map, so the buffer-end fallback is only ever reachable as a wrong answer.
fn find_visual_line_at(map: &[VisualLine], line: usize, col: usize) -> usize {
    let mut fallback = 0;
    for (i, vl) in map.iter().enumerate() {
        if vl.logical != line {
            continue;
        }
        fallback = i;
        if col >= vl.start && col < vl.start + vl.len {
            return i;
        }
    }
    fallback
}

/// Whether visual line `i` is the LAST segment of its logical line — the final map entry, or
/// followed by an entry belonging to a different logical line (`editor.ts:1407-1409`, `:1413-1415`).
fn is_last_segment(map: &[VisualLine], i: usize) -> bool {
    match (map.get(i), map.get(i.saturating_add(1))) {
        (Some(vl), Some(next)) => next.logical != vl.logical,
        _ => true,
    }
}

/// The highest visual column the caret may occupy on visual line `i` —
/// `isLastSegment ? vl.length : Math.max(0, vl.length - 1)` (`editor.ts:1410`, `:1416`).
///
/// The `- 1` matters: on a NON-last segment, column `start + len` is the wrap boundary, which
/// belongs to the *next* visual row (see [`find_visual_line_at`]). The movers previously clamped
/// with a bare `goal.min(vl.len)` and so could park the caret on a column that renders one row down.
fn segment_max_visual_col(map: &[VisualLine], i: usize) -> usize {
    let len = map.get(i).map_or(0, |vl| vl.len);
    if is_last_segment(map, i) {
        len
    } else {
        len.saturating_sub(1)
    }
}

/// The **display width** of `s` in terminal cells — Pi's `visibleWidth` (`utils.ts:240-...`), which
/// is what every wrap and caret computation upstream measures with. `Span::width()` is ratatui's
/// `unicode_width` sum, the same primitive `transcript.rs` already uses for `Box.applyBg`.
///
/// Not `chars().count()`: a CJK ideograph is one `char` and TWO columns, a combining mark is one
/// `char` and ZERO, so a char count is neither an upper nor a lower bound on the cells a string
/// occupies.
pub(super) fn display_width(s: &str) -> usize {
    Span::raw(s).width()
}

/// Whether a grapheme counts as whitespace — Pi's `isWhitespaceChar`, which is literally
/// `/\s/.test(char)` (`utils.ts:943-945`), i.e. *contains* a whitespace code point rather than *is*
/// one, hence `any` and not `all`.
///
/// [CYRUP-DELTA] JS `\s` and Rust's `char::is_whitespace` (Unicode `White_Space`) differ on exactly
/// two code points in either direction: `\s` includes U+FEFF (which `White_Space` does not) and
/// `White_Space` includes U+0085 NEL (which `\s` does not). Both are stripped before they can reach
/// the buffer — U+FEFF and U+0085 are `char::is_control()`/format characters that
/// `sanitize_paste` drops and that no key event produces — so the sets coincide on every input
/// this function can actually see.
fn is_whitespace_seg(g: &str) -> bool {
    g.chars().any(char::is_whitespace)
}

/// Whether a grapheme offers a CJK line-break opportunity — Pi's `cjkBreakRegex` (`utils.ts:54-55`):
///
/// ```text
/// /[\p{Script_Extensions=Han}\p{Script_Extensions=Hiragana}\p{Script_Extensions=Katakana}
///   \p{Script_Extensions=Hangul}\p{Script_Extensions=Bopomofo}]/u
/// ```
///
/// Tested against the whole grapheme (the regex is unanchored, so it matches if ANY code point in
/// the cluster qualifies) — hence `any`, matching `cjkBreakRegex.test(grapheme)`.
///
/// [CYRUP-DELTA] Rust's standard library carries no `Script_Extensions` data and pulling in a
/// unicode-script crate for one predicate would be a new external dependency for a table that does
/// not move (these five scripts' blocks are stable since Unicode 13). The ranges below are the
/// assigned blocks of those five scripts plus the shared-ideographic code points
/// (`〄〇〡-〩〸-〻`) that `Script_Extensions=Han` picks up beyond `Script=Han`.
fn is_cjk_break(g: &str) -> bool {
    g.chars().map(u32::from).any(|c| {
        matches!(c,
            0x1100..=0x11FF   // Hangul Jamo
            | 0x2E80..=0x2EFF // CJK Radicals Supplement       (Han)
            | 0x2F00..=0x2FDF // Kangxi Radicals               (Han)
            | 0x3005          // 々 ideographic iteration mark (Han)
            | 0x3007          // 〇 ideographic number zero    (Han)
            | 0x3021..=0x3029 // 〡-〩 Hangzhou numerals        (Han)
            | 0x3038..=0x303B // 〸-〻                          (Han)
            | 0x3041..=0x309F // Hiragana (incl. the shared voiced-sound marks)
            | 0x30A0..=0x30FF // Katakana
            | 0x3100..=0x312F // Bopomofo
            | 0x3130..=0x318F // Hangul Compatibility Jamo
            | 0x31A0..=0x31BF // Bopomofo Extended
            | 0x31F0..=0x31FF // Katakana Phonetic Extensions
            | 0x3400..=0x4DBF // CJK Unified Ideographs Extension A
            | 0x4E00..=0x9FFF // CJK Unified Ideographs
            | 0xA960..=0xA97F // Hangul Jamo Extended-A
            | 0xAC00..=0xD7AF // Hangul Syllables
            | 0xD7B0..=0xD7FF // Hangul Jamo Extended-B
            | 0xF900..=0xFAFF // CJK Compatibility Ideographs
            | 0xFE30..=0xFE4F // CJK Compatibility Forms       (Han)
            | 0xFF66..=0xFF9F // Halfwidth Katakana
            | 0xFFA0..=0xFFDC // Halfwidth Hangul
            | 0x1B000..=0x1B16F // Kana Supplement / Extended-A / Small Kana Extension
            | 0x20000..=0x2FA1F // CJK Extensions B-F + Compatibility Supplement
            | 0x30000..=0x323AF // CJK Extensions G-H
        )
    })
}

/// Word-aware wrap of one logical line into `(start_col, len)` visual segments fitting `width`
/// **display columns** — a 1:1 port of `wordWrapLine` (`editor.ts:114-206`). An empty line yields
/// one zero-length segment. `width` is assumed `>= 1` (callers clamp). The returned columns are
/// char indices into `line` (cyrup's buffer is `Vec<char>`, so char indices are its `string.slice`),
/// and the segments tile the line contiguously.
///
/// The three things the previous implementation got wrong, all from measuring `n - start <= width`
/// over a `&[char]` — a CHAR COUNT:
///
/// 1. **Width.** Upstream accumulates `visibleWidth(grapheme)` (`:139-143`), so 24 CJK ideographs
///    are 48 columns, not 24. At a layout width of 39 the char count said "fits", the map reported
///    one visual line, four ideographs rendered past the right edge and — because
///    [`crate::editor::InputEditor::cursor_in`] resolves the caret through that same map — the caret left the frame.
/// 2. **Granularity.** Upstream iterates GRAPHEMES and breaks at a cluster's own start index
///    (`:157-160`), so a break never lands inside a cluster. Breaking at `start + width` char-wise
///    put `👨` on one row and a bare `\u{200d}👩‍👧‍👦` on the next.
/// 3. **CJK break opportunities.** Upstream records a wrap opportunity between any two adjacent
///    non-space graphemes when either is CJK (`:191-198`), because CJK text has no spaces to break
///    at. Without it a whole CJK paragraph is one unbreakable "word".
///
/// The loop below is upstream's, statement for statement: an overflow check that first tries to
/// backtrack to the last recorded opportunity and otherwise force-breaks at the current cluster's
/// start (`:145-161`), then the advance and the opportunity bookkeeping (`:180-199`).
///
/// [CYRUP-DELTA] `:163-178` handles a single segment wider than `maxWidth` by *recursively*
/// re-wrapping it, which upstream needs because its segmenter merges a whole `[paste #N …]` marker
/// into one atomic segment. cyrup's segments are plain extended grapheme clusters — never composite
/// — so there is nothing to re-wrap: an over-wide cluster (a wide emoji at `width == 1`) is
/// indivisible and takes a row of its own. That is where upstream's recursion converges for a
/// splittable segment, and it is also the case upstream cannot express at all: `wordWrapLine("👨",
/// 1)` recurses on itself forever.
pub(super) fn word_wrap_line(line: &[char], width: usize) -> Vec<(usize, usize)> {
    let width = width.max(1);
    let n = line.len();
    // `if (!line || maxWidth <= 0) return [{ text: "", startIndex: 0, endIndex: 0 }]` (`:115-117`).
    if n == 0 {
        return vec![(0, 0)];
    }
    let s: String = line.iter().collect();
    // `if (lineWidth <= maxWidth) return [{ text: line, ... }]` (`:119-122`).
    if display_width(&s) <= width {
        return vec![(0, n)];
    }

    // `const segments = [...graphemeSegmenter.segment(line)]` (`:125`), carrying each cluster's
    // start index — `seg.index` upstream, a char column here.
    let mut segs: Vec<(usize, &str)> = Vec::with_capacity(n);
    let mut col = 0usize;
    for g in s.graphemes(true) {
        segs.push((col, g));
        col += g.chars().count();
    }

    let mut chunks: Vec<(usize, usize)> = Vec::new();
    let mut current_width = 0usize;
    let mut chunk_start = 0usize;
    // `wrapOppIndex` / `wrapOppWidth` (`:131-133`), as one `Option` so `-1` cannot leak.
    let mut wrap_opp: Option<(usize, usize)> = None;

    for i in 0..segs.len() {
        let Some(&(char_index, grapheme)) = segs.get(i) else {
            continue;
        };
        let g_width = display_width(grapheme);
        let is_ws = is_whitespace_seg(grapheme);

        // "Overflow check before advancing" (`:145-161`).
        if current_width + g_width > width {
            match wrap_opp {
                // "Backtrack to last wrap opportunity (the remaining content plus the current
                // grapheme still fits within maxWidth)" (`:147-153`).
                Some((opp_index, opp_width))
                    if current_width.saturating_sub(opp_width) + g_width <= width =>
                {
                    chunks.push((chunk_start, opp_index.saturating_sub(chunk_start)));
                    chunk_start = opp_index;
                    current_width = current_width.saturating_sub(opp_width);
                }
                // "No viable wrap opportunity: force-break at current position" (`:154-160`).
                _ if chunk_start < char_index => {
                    chunks.push((chunk_start, char_index - chunk_start));
                    chunk_start = char_index;
                    current_width = 0;
                }
                _ => {}
            }
            wrap_opp = None;
        }

        // `if (gWidth > maxWidth)` (`:163`) — see the [CYRUP-DELTA] above.
        if g_width > width {
            if chunk_start < char_index {
                chunks.push((chunk_start, char_index - chunk_start));
            }
            chunk_start = char_index;
            current_width = g_width;
            wrap_opp = None;
            continue;
        }

        // "Advance" (`:181`).
        current_width += g_width;

        // "Record wrap opportunity" (`:183-199`): whitespace followed by non-whitespace (multiple
        // spaces join; the break point is after the last space), or a boundary where either side is
        // CJK.
        if let Some(&(next_index, next)) = segs.get(i + 1)
            // Upstream spells this as two arms — whitespace→non-whitespace (`:187-189`) and the CJK
            // boundary (`:190-198`) — that assign the same pair. Merged into one predicate because
            // clippy's `if_same_then_else` rejects the duplicated arm; `is_ws || cjk || cjk` under a
            // shared `!next_is_ws` is exactly the disjunction of the two upstream guards.
            && !is_whitespace_seg(next)
            && (is_ws || is_cjk_break(grapheme) || is_cjk_break(next))
        {
            wrap_opp = Some((next_index, current_width));
        }
    }

    // "Push final chunk" (`:202`).
    chunks.push((chunk_start, n.saturating_sub(chunk_start)));
    chunks
}

/// The grapheme-cluster boundaries of `line` expressed as **char-column** indices, including the
/// leading `0` and trailing line length (`unicode_segmentation` extended grapheme clusters — the
/// boundaries Pi's editor steps the cursor over). A pure-ASCII line yields every column.
pub(super) fn grapheme_boundaries(line: &[char]) -> Vec<usize> {
    let s: String = line.iter().collect();
    let mut bounds = Vec::with_capacity(line.len() + 1);
    bounds.push(0usize);
    let mut col = 0usize;
    for g in s.graphemes(true) {
        col += g.chars().count();
        bounds.push(col);
    }
    bounds
}
