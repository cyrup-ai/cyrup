use super::*;

impl<'t> MdRenderer<'t> {
    /// Emit the captured table as a full box-drawing grid (`┌┬┐ ├┼┤ └┴┘ │ ─`), a port of
    /// `markdown.ts:837-1009` `renderTable`: per-column width fitting to the content width, a bold
    /// header band, and width-aware cell wrapping. Closes gap 12 (tables were previously
    /// ` │ `-joined, no grid).
    ///
    /// Exactly **one** `├─┼─┤` separator follows the header band (`markdown.ts:975-977`), and after
    /// that a separator is emitted only BETWEEN body rows — `if (rowIndex < token.rows.length - 1)
    /// lines.push(separatorLine)` (`:996-998`) — so the last body row butts straight onto the
    /// `└─┴─┘`. (An earlier revision of this comment claimed a separator between *every* row; the
    /// code never did that, only the prose did.)
    ///
    /// The frame is UNSTYLED upstream, so it takes the body colour; header cells are body colour +
    /// bold; body cells the base style. A pane too narrow for the grid degrades to the raw Markdown
    /// (`markdown.ts:853-861`).
    pub(super) fn emit_table(&mut self) {
        let Some(t) = self.table.take() else { return };
        let num_cols = t.header.len();
        if num_cols == 0 {
            return;
        }
        // `renderTable(token, width, …)` (`markdown.ts:551`) is handed the width `renderToken` was
        // called with — `itemWidth` inside a list item (`:786`), `quoteContentWidth` inside a
        // blockquote (`:583`) — so the grid, its too-narrow guard and the raw fallback are all sized
        // to the CONTAINER, not to the component.
        let avail_width = self.content_width();
        // Border overhead = "│ " + (n-1)*" │ " + " │" = 3n + 1 (`markdown.ts:850-852`).
        let overhead = 3usize.saturating_mul(num_cols).saturating_add(1);
        // `const availableForCells = availableWidth - borderOverhead; if (availableForCells <
        // numCols) { … return token.raw ? wrapTextWithAnsi(token.raw, availableWidth) : []; }`
        // (`markdown.ts:853-861`) — too narrow for a stable grid, so degrade to the raw Markdown
        // instead of drawing a grid wider than the pane. Signed there, saturating here: the guard is
        // `width < overhead + numCols`.
        if avail_width < overhead.saturating_add(num_cols) {
            let style = self.theme.assistant_style();
            // `wrapTextWithAnsi(token.raw, availableWidth)` (`markdown.ts:856`) — the raw source is
            // WRAPPED to the pane, not pushed through at its natural width, so the fallback never
            // draws wider than the grid it replaced. `wrapTextWithAnsi` splits on `\n` first
            // (`utils.ts:838-839`) and word-wraps each resulting line, which is what running
            // [`wrap_cell`] per source line does here.
            //
            // Routed through `push_text` + `flush_line` rather than straight onto `self.out` so the
            // rows collect the same `│ `/indent prefixes as everything else: upstream the fallback is
            // a plain `string[]` return whose caller prefixes it — `this.theme.quoteBorder("│ ") +
            // wrappedLine` for a blockquote (`markdown.ts:596`), `linePrefix + wrappedLine` for a
            // list item (`:790`).
            for src in t.raw.trim_end_matches('\n').split('\n') {
                // marked hands `renderTable` a `token.raw` whose blockquote markers are already gone
                // — the `blockquote` tokenizer strips them before re-lexing the body — where
                // pulldown-cmark's offset range is a slice of the untouched source. Without this the
                // fallback printed `> | Name | Role |`, leaking the quote syntax INSIDE the `│ `
                // border this batch had just taught it to draw. See [`strip_quote_markers`].
                let src = strip_quote_markers(src, self.quote);
                for row in wrap_cell(&[Span::styled(src.to_string(), style)], avail_width) {
                    if row.spans.is_empty() {
                        // `breakLongWord`'s unguarded flush emits a genuinely empty row; keep it as
                        // a row rather than letting `flush_line`'s empty-`cur` guard swallow it.
                        self.push_text("", style);
                    }
                    for span in row.spans {
                        self.push_text(span.content.as_ref(), span.style);
                    }
                    self.flush_line();
                }
            }
            // `if (nextTokenType && nextTokenType !== "space") fallbackLines.push("")` (`:857-859`).
            self.blank();
            return;
        }
        let avail_cells = avail_width.saturating_sub(overhead);

        // Natural width per column = widest VISIBLE cell (header + body). No floor here: upstream's
        // `naturalWidths[i] = visibleWidth(headerText)` / `Math.max(naturalWidths[i] || 0,
        // visibleWidth(cellText))` (`markdown.ts:870`, `:876`) is unfloored, and the ≥1 guarantee
        // arrives via `minColumnWidths` below (`:919`). Measured on the STYLED cell — `Line::width`,
        // never `chars().count()`.
        //
        // M15: alongside it upstream computes a per-column MINIMUM from the longest unbroken word,
        // capped at `const maxUnbrokenWordWidth = 30` (`markdown.ts:863`, `:871`, `:877-880`). That
        // floor is what stops a column from being squeezed to one cell and shredding every word in
        // it one grapheme per row; cyrup floored at 1 unconditionally and lost it.
        let mut natural: Vec<usize> = t.header.iter().map(|h| spans_width(h)).collect();
        let mut min_word: Vec<usize> = t
            .header
            .iter()
            // `Math.max(1, this.getLongestWordWidth(headerText, maxUnbrokenWordWidth))` (`:871`) —
            // the `max(1, …)` is on the HEADER pass only; the row pass at `:877-880` maxes against
            // whatever the header left, so the floor propagates.
            .map(|h| longest_word_width(h).clamp(1, MAX_UNBROKEN_WORD_WIDTH))
            .collect();
        for row in &t.rows {
            // `zip` is the index-free spelling of upstream's `for (let i = 0; i < row.length; i++)`
            // (`:874`): a malformed row with more cells than the header has columns contributes
            // nothing past the last column, exactly as `naturalWidths[i]` would stay `undefined`.
            for ((nat, minw), cell) in
                natural.iter_mut().zip(min_word.iter_mut()).zip(row.iter())
            {
                *nat = (*nat).max(spans_width(cell));
                *minw = (*minw).max(longest_word_width(cell).min(MAX_UNBROKEN_WORD_WIDTH));
            }
        }

        // `let minColumnWidths = minWordWidths; … if (minCellsWidth > availableForCells) { … }`
        // (`markdown.ts:884-911`). Only when the word floors TOGETHER overflow the row does upstream
        // give up on them, collapse to all-1s and hand the slack back in proportion to how much each
        // column wanted (`:888-908`) — so a narrow pane degrades gracefully instead of every column
        // being 1 the moment the table does not fit naturally.
        let mut min_cols: Vec<usize> = min_word.clone();
        let mut min_cells: usize = min_cols.iter().sum();
        if min_cells > avail_cells {
            min_cols = vec![1usize; num_cols];
            let remaining = avail_cells.saturating_sub(num_cols);
            if remaining > 0 {
                // `totalWeight = Σ max(0, width - 1)`, `growth[i] = floor((weight / totalWeight) *
                // remaining)` (`:892-896`). Integer `weight * remaining / totalWeight` is the same
                // floor without the float round-trip.
                let total_weight: usize = min_word.iter().map(|w| w.saturating_sub(1)).sum();
                let growth: Vec<usize> = min_word
                    .iter()
                    .map(|w| {
                        let weight = w.saturating_sub(1);
                        weight.saturating_mul(remaining).checked_div(total_weight).unwrap_or(0)
                    })
                    .collect();
                for (m, g) in min_cols.iter_mut().zip(growth.iter()) {
                    *m = m.saturating_add(*g);
                }
                // `for (let i = 0; leftover > 0 && i < numCols; i++) minColumnWidths[i]++` (`:904-907`)
                // — the rounding remainder goes left to right, one cell per column, ONE pass.
                let allocated: usize = growth.iter().sum();
                let mut leftover = remaining.saturating_sub(allocated);
                for m in min_cols.iter_mut() {
                    if leftover == 0 {
                        break;
                    }
                    *m = m.saturating_add(1);
                    leftover -= 1;
                }
            }
            min_cells = min_cols.iter().sum();
        }

        // Fit: `totalNaturalWidth = Σ naturalWidths + borderOverhead; if (totalNaturalWidth <=
        // availableWidth)` (`:914-919`) — identical to comparing `Σ natural` against `avail_cells`.
        let total_natural: usize = natural.iter().sum();
        let widths: Vec<usize> = if total_natural <= avail_cells {
            // `columnWidths = naturalWidths.map((w, i) => Math.max(w, minColumnWidths[i]))` (`:919`).
            natural.iter().zip(min_cols.iter()).map(|(n, m)| (*n).max(*m)).collect()
        } else {
            // Shrink toward `minColumnWidths`, NOT toward 1 (`:920-934`).
            let grow_potential: usize =
                natural.iter().zip(min_cols.iter()).map(|(n, m)| n.saturating_sub(*m)).sum();
            let extra = avail_cells.saturating_sub(min_cells);
            let mut w: Vec<usize> = natural
                .iter()
                .zip(min_cols.iter())
                .map(|(n, m)| {
                    let delta = n.saturating_sub(*m);
                    let grow =
                        delta.saturating_mul(extra).checked_div(grow_potential).unwrap_or(0);
                    m.saturating_add(grow)
                })
                .collect();
            // Distribute rounding leftovers left-to-right, never past the natural width (`:936-951`).
            let mut remaining = avail_cells.saturating_sub(w.iter().sum());
            loop {
                let mut grew = false;
                for (wi, ni) in w.iter_mut().zip(natural.iter()) {
                    if remaining == 0 {
                        break;
                    }
                    if *wi < *ni {
                        *wi += 1;
                        remaining -= 1;
                        grew = true;
                    }
                }
                if !grew || remaining == 0 {
                    break;
                }
            }
            w
        };

        // The grid is drawn with NO theme function at all upstream — `` `┌─${…join("─┬─")}─┐` ``
        // (`markdown.ts:956`), `` `│ ${rowParts.join(" │ ")} │` `` (`:971`), `` `├─…─┼─…─┤` ``
        // (`:976`) and `` `└─…─┴─…─┘` `` (`:1003`) are plain template strings, so the frame renders
        // in the same colour as body prose rather than in `mdHr`.
        let base = self.theme.assistant_style();
        let border = |left: &str, mid: &str, right: &str, ws: &[usize]| -> Line<'static> {
            let cells: Vec<String> = ws.iter().map(|w| "─".repeat(*w)).collect();
            Line::styled(format!("{left}{}{right}", cells.join(mid)), base)
        };

        // EVERY row — frame and grid alike — leaves through [`Self::emit_prefixed`], for the same
        // reason the fallback above does. Upstream `renderTable` returns a bare `string[]`
        // (`markdown.ts:1005`) and its caller decorates it: `quoteBorder("│ ") + wrappedLine` for a
        // blockquote (`:596`), `linePrefix + wrappedLine` for a list item (`:790`). Pushing straight
        // onto `self.out` cost two things at once — a table inside a blockquote lost its border, and
        // a table that was the first block of a `- ` item swallowed the queued bullet outright,
        // because only `open_line()` consumes `pending_marker` and the next `Start(Item)` overwrote
        // whatever it left behind.
        //
        // The wrap `emit_prefixed` adds is a no-op here by construction: `avail_width` is already
        // `content_width()`, i.e. `self.width` minus the very prefix `flush_line` will measure.

        // Top border ┌─...─┬─...─┐.
        self.emit_prefixed(border("┌─", "─┬─", "─┐", &widths));
        // Header band (bold), wrapped.
        self.push_table_row(&t.header, &widths, true, '│');
        // Separator ├─...─┼─...─┤.
        let sep = || border("├─", "─┼─", "─┤", &widths);
        self.emit_prefixed(sep());
        for (ri, row) in t.rows.iter().enumerate() {
            self.push_table_row(row, &widths, false, '│');
            if ri + 1 < t.rows.len() {
                self.emit_prefixed(sep());
            }
        }
        // Bottom border └─...─┴─...─┘.
        self.emit_prefixed(border("└─", "─┴─", "─┘", &widths));
        self.blank();
    }

    /// Render one table row of `cells` into `│ … │` lines, wrapping each cell to its column width and
    /// padding short cells with spaces (`markdown.ts:958-994`). The `│` separators are **unstyled**
    /// upstream (`:971` is a plain template string), so they take the same body colour as the rest of
    /// the frame.
    ///
    /// M7: the cells arrive already styled by the inline walk, so each cell's own spans are carried
    /// through verbatim. `bold` is upstream's header band — `return this.theme.bold(padded)`
    /// (`:966-970`), i.e. SGR-1 wrapped around the whole padded cell, adding NO foreground of its
    /// own; a header cell therefore stays whatever colour its inline run gave it AND gains bold,
    /// which is why the modifier is added per span rather than replacing the span's style.
    ///
    /// The wrap is [`crate::transcript::wrap_line`] because upstream's `wrapCellText` is literally
    /// `wrapTextWithAnsi(text, Math.max(1, maxWidth))` (`markdown.ts:829-831`) — the SAME primitive
    /// prose goes through, and it is ANSI-aware precisely so a styled cell wraps without shredding
    /// its escapes. A plain-`str` cell wrapper cannot preserve per-span styles at all.
    fn push_table_row(&mut self, cells: &[CellSpans], widths: &[usize], bold: bool, bar: char) {
        let bar_style = self.theme.assistant_style();
        let pad_style =
            if bold { bar_style.add_modifier(Modifier::BOLD) } else { bar_style };
        let empty: CellSpans = Vec::new();
        let wrapped: Vec<Vec<Line<'static>>> = widths
            .iter()
            .zip(cells.iter().chain(std::iter::repeat(&empty)))
            .map(|(w, cell)| wrap_cell(cell, *w))
            .collect();
        let height = wrapped.iter().map(Vec::len).max().unwrap_or(1).max(1);
        for li in 0..height {
            let mut spans: Vec<Span<'static>> = Vec::with_capacity(widths.len() * 3 + 2);
            spans.push(Span::styled(format!("{bar} "), bar_style));
            for (ci, (w, cell_rows)) in widths.iter().zip(wrapped.iter()).enumerate() {
                if ci > 0 {
                    spans.push(Span::styled(format!(" {bar} "), bar_style));
                }
                let row = cell_rows.get(li);
                let text_w = row.map(Line::width).unwrap_or(0);
                if let Some(row) = row {
                    spans.extend(row.spans.iter().map(|s| {
                        let st = if bold { s.style.add_modifier(Modifier::BOLD) } else { s.style };
                        Span::styled(s.content.clone().into_owned(), st)
                    }));
                }
                // `text + " ".repeat(Math.max(0, columnWidths[colIdx] - visibleWidth(text)))`
                // (`:968`, `:991`) — the pad is OUTSIDE the cell's own styling, inside `theme.bold`
                // for the header band.
                let pad = w.saturating_sub(text_w);
                if pad > 0 {
                    spans.push(Span::styled(" ".repeat(pad), pad_style));
                }
            }
            spans.push(Span::styled(format!(" {bar}"), bar_style));
            // Through the prefix machinery, never `self.out.push` — see [`Self::emit_table`].
            self.emit_prefixed(Line::from(spans));
        }
    }
}

/// `const maxUnbrokenWordWidth = 30` (`markdown.ts:863`) — the cap M15 restores.
const MAX_UNBROKEN_WORD_WIDTH: usize = 30;

/// The visible text of a styled cell, escapes excluded — the input `getLongestWordWidth` splits.
fn cell_text(cell: &[Span<'static>]) -> String {
    cell.iter().map(|s| s.content.as_ref()).collect()
}

/// Widest single whitespace-delimited word of a cell, in terminal columns.
///
/// `getLongestWordWidth` (`markdown.ts:811-821`): `text.split(/\s+/).filter(w => w.length > 0)` then
/// `Math.max(…visibleWidth(word))`, and the caller's `maxWidth` cap is applied by the CALLER's
/// `.min()` here so the `Math.max(1, …)` / `Math.max(prev, …)` asymmetry at `:871` vs `:877-880`
/// stays visible at the call sites.
fn longest_word_width(cell: &[Span<'static>]) -> usize {
    cell_text(cell).split_whitespace().map(display_width).max().unwrap_or(0)
}

/// `String::trim` lifted to a styled run: drop leading/trailing whitespace across span boundaries,
/// keeping every surviving span's style.
///
/// The trim exists because pulldown-cmark hands `Event::Text` the cell's source slice INCLUDING the
/// `| ` padding spaces, where marked's `splitCells` has already stripped them before
/// `renderInlineTokens` ever sees the cell.
pub(super) fn trim_cell(cell: CellSpans) -> CellSpans {
    let mut out: CellSpans = cell;
    while let Some(first) = out.first_mut() {
        let trimmed = first.content.trim_start().to_string();
        if trimmed.is_empty() {
            out.remove(0);
        } else {
            first.content = trimmed.into();
            break;
        }
    }
    while let Some(last) = out.last_mut() {
        let trimmed = last.content.trim_end().to_string();
        if trimmed.is_empty() {
            out.pop();
        } else {
            last.content = trimmed.into();
            break;
        }
    }
    out
}

/// Greedy word-wrap `text` to a column of `width` cells — Pi's `wrapCellText` (`markdown.ts:829-831`),
/// which is `wrapTextWithAnsi(text, Math.max(1, maxWidth))`: tokenize into alternating
/// whitespace/word runs, pack tokens onto a line, and hard-break any single word wider than the
/// column. Always returns at least one (possibly empty) line so a cell occupies a row.
///
/// The whitespace between two words is a **token that is carried through**, not a separator the
/// wrapper regenerates (`utils.ts:775-798` tokenizes, `:923` appends verbatim). That matters for M7
/// precisely because a cell is styled: `renderInlineTokens` emits the inter-word space OUTSIDE the
/// SGR pairs of either neighbour, so in `**a** *b*` the gap is ambient-styled — synthesizing it from
/// `line.last()`'s style painted it with the preceding word's bold, and collapsing runs of spaces
/// silently reflowed `a  b` to `a b`.
///
/// The long-word break walks **extended grapheme clusters**, not `char`s — Pi's `breakLongWord`
/// segments with `graphemeSegmenter.segment(textPortion)` and advances one `seg.segment` at a time
/// (`tui/src/utils.ts:977-979`, `:986-1013`). A `char` walk splits a ZWJ emoji family between its
/// members and detaches a combining mark from its base, both of which corrupt the cell.
///
/// **M7: the unit is a styled run, not a `str`.** Upstream feeds `wrapCellText` the output of
/// `renderInlineTokens` — an ANSI-carrying string — and `breakLongWord` is written to walk past
/// escapes precisely so a bold or `code`-styled cell survives the break (`utils.ts:958-983`). The
/// port carries the style on the grapheme instead, which is the same guarantee with no escapes to
/// step over.
///
/// This is NOT a second copy of [`crate::transcript::wrap_line`] to be folded into it: the two
/// disagree on one documented point. `breakLongWord`'s flush at `utils.ts:1000` is **unguarded**, so
/// a cluster wider than the whole column pushes an EMPTY row first; `wrap_line` guards that flush
/// with `!cur.is_empty()` and produces no leading empty row. Upstream's shape is the unguarded one —
/// it is what the hand-traced width-13 CJK expectation in `tests/markdown.rs` pins, glyph for glyph.
fn wrap_cell(cell: &[Span<'static>], width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    // `if (visibleLength <= width) return [line];` (`utils.ts:1006-1009`) — a line that already fits
    // is returned VERBATIM, keeping its interior spacing rather than being re-packed word by word.
    if spans_width(cell) <= width {
        return vec![Line::from(cell.to_vec())];
    }
    // Flatten to (grapheme, style) so a word may straddle a span boundary — `**bo**ld` is two spans
    // and one word, and the break point does not care which.
    let mut graphemes: Vec<(&str, Style)> = Vec::new();
    for span in cell {
        let st = span.style;
        graphemes.extend(span.content.graphemes(true).map(|g| (g, st)));
    }
    // `splitIntoTokensWithAnsi` (`utils.ts:775-798`) emits ALTERNATING whitespace and
    // non-whitespace runs, and `wrapSingleLine` then appends each token VERBATIM
    // (`currentLine += token`, `:923`). Whitespace is a token, never a separator the wrapper is
    // free to regenerate: the run between `**a**` and `*b*` is the source space, which
    // `renderInlineTokens` leaves OUTSIDE both SGR pairs (`\x1b[1ma\x1b[22m \x1b[3mb\x1b[23m`), so
    // it carries the ambient style — not the preceding word's bold. Re-inserting a single `" "`
    // with `line.last()`'s style got that wrong in both directions: it bolded the gap after a bold
    // word, and it collapsed `a  b` to `a b`.
    let mut tokens: Vec<Vec<(&str, Style)>> = Vec::new();
    for g in graphemes {
        let ws = is_ws_grapheme(g.0);
        match tokens.last_mut() {
            Some(tok) if tok.first().is_some_and(|f| is_ws_grapheme(f.0) == ws) => tok.push(g),
            _ => tokens.push(vec![g]),
        }
    }

    let mut out: Vec<Vec<(&str, Style)>> = Vec::new();
    let mut line: Vec<(&str, Style)> = Vec::new();
    let mut line_w = 0usize;
    for word in tokens {
        let ww: usize = word.iter().map(|(g, _)| display_width(g)).sum();
        let is_ws = word.first().is_some_and(|(g, _)| is_ws_grapheme(g));
        // `if (tokenVisibleLength > width && !isWhitespace)` (`:876`) — a run of spaces wider than
        // the column is never hard-broken; it is dropped at the fold like any other.
        if ww > width && !is_ws {
            // Flush the current line, then hard-break the long word into width-sized chunks.
            if !line.is_empty() {
                out.push(std::mem::take(&mut line));
                line_w = 0;
            }
            let mut chunk: Vec<(&str, Style)> = Vec::new();
            let mut chunk_w = 0usize;
            for g in word {
                let cw = display_width(g.0);
                // `if (currentWidth + graphemeWidth > width) { lines.push(currentLine); … }`
                // (`utils.ts:1000-1010`) — unguarded upstream, so a cluster WIDER than the column
                // (a CJK ideograph in a 1-cell column) flushes an empty row and is then emitted
                // whole. Clusters are never split below the cluster, so the row overflows by design
                // rather than dropping half a glyph; `Math.max(0, …)` on the pad (`:991`) is what
                // keeps that from underflowing upstream, and `saturating_sub` here.
                if chunk_w + cw > width {
                    out.push(std::mem::take(&mut chunk));
                    chunk_w = 0;
                }
                chunk.push(g);
                chunk_w += cw;
            }
            if !chunk.is_empty() {
                line = chunk;
                line_w = chunk_w;
            }
            continue;
        }
        // `if (totalNeeded > width && currentVisibleLength > 0)` (`:903`) — fold, and then
        // `if (isWhitespace) { currentLine = ""; }` (`:911-913`): "Don't start new line with
        // whitespace", i.e. the separator token is CONSUMED by the fold rather than opening the
        // next row with an indent.
        if line_w + ww > width && line_w > 0 {
            out.push(std::mem::take(&mut line));
            line_w = 0;
            if is_ws {
                continue;
            }
        }
        line.extend(word);
        line_w += ww;
    }
    // `if (currentLine) { wrapped.push(currentLine); }` (`:927-931`) — guarded, then
    // `wrapped.length > 0 ? … : [""]` (`:935`) keeps the cell at one row minimum.
    if !line.is_empty() {
        out.push(line);
    }
    if out.is_empty() {
        out.push(Vec::new());
    }
    out.into_iter()
        .map(|mut row| {
            // `wrapped.map((line) => line.trimEnd())` (`:935`) — "Trailing whitespace can cause
            // lines to exceed the requested width". Only reachable now that the separator survives
            // into the row instead of being synthesized between words.
            while row.last().is_some_and(|(g, _)| is_ws_grapheme(g)) {
                row.pop();
            }
            let mut spans: Vec<Span<'static>> = Vec::new();
            for (g, st) in row {
                match spans.last_mut() {
                    Some(last) if last.style == st => last.content.to_mut().push_str(g),
                    _ => spans.push(Span::styled(g.to_string(), st)),
                }
            }
            Line::from(spans)
        })
        .collect()
}
