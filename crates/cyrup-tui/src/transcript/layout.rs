use super::*;

/// Visible width of one **grapheme cluster** in terminal columns, via ratatui's
/// unicode-width-backed `Span::width` (Pi's `graphemeWidth`, `utils.ts:174-235`).
///
/// Deliberately the same measure [`apply_bg`] reaches through [`Line::width`]: L6's defect was that
/// the wrap point and the background pad disagreed, so the fix is only a fix if both halves count
/// the same way. `Span::width` sums `unicode_width` per `char`, so summing it per grapheme over a
/// span is identical to calling it once on the whole span — the two can never drift.
fn grapheme_cols(g: &str) -> usize {
    Span::raw(g).width()
}

/// Whether a grapheme cluster is whitespace (`token.trim() === ""`, `utils.ts:876`).
pub(crate) fn is_ws_grapheme(g: &str) -> bool {
    g.chars().all(char::is_whitespace)
}

/// Greedy word-wrap one styled line into `width`-column rows, preserving each span's style.
///
/// Port of `wrapSingleLine` (`tui/src/utils.ts:857-936`), which `Text`/`Markdown` run before a
/// `Box` re-pads them and which is therefore what makes `box.ts:85`'s `child.render(contentWidth)`
/// actually produce `contentWidth`-wide rows. Upstream's `AnsiCodeTracker` plumbing
/// (`:868`, `:914`, `:917`) is deliberately absent: a terminal line break clears the active SGR
/// state, so upstream must re-emit it at the head of every produced row, whereas ratatui carries
/// style per `Span` and the styles travel with the characters.
///
/// Semantics kept verbatim: an over-wide single token is hard-broken (`breakLongWord`, `:879-899`),
/// a wrap never starts a row with whitespace (`:912-915`), and every produced row is right-trimmed
/// (`:935`).
///
/// **The unit is the grapheme cluster, never the `char`.** Both of upstream's loops run over
/// `graphemeSegmenter.segment(...)`: `splitIntoTokensWithAnsi` builds its tokens grapheme by
/// grapheme (`utils.ts:775-798`) and `breakLongWord` segments the over-wide token the same way
/// before measuring it (`:979-980`, `:994-1012`). Wrapping per `char` instead tears a ZWJ emoji
/// sequence or a combining mark away from its base — a correctness bug, not a spacing one — and it
/// also measures differently from [`apply_bg`], which is exactly the disagreement L6 is about.
///
/// `pub(crate)` because [`crate::markdown`] is upstream's THIRD consumer of the same primitive —
/// `wrapTextWithAnsi` is called from `text.ts:67`, `box.ts` (transitively, through its child) AND
/// `markdown.ts:322`/`:594`/`:788`. There is exactly one wrapper here for the same reason there is
/// exactly one upstream; do not grow a fourth.
pub(crate) fn wrap_line(line: &Line<'static>, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    if line.width() <= width {
        return vec![line.clone()];
    }
    // Flatten to (grapheme, style) cells so a word may straddle a span boundary. Upstream segments
    // per non-ANSI run, i.e. per style run, which is exactly per `Span` here (`utils.ts:770-798`).
    let mut cells: Vec<(&str, Style)> = Vec::new();
    for span in &line.spans {
        let st = span.style;
        cells.extend(span.content.graphemes(true).map(|g| (g, st)));
    }
    // Tokenize into alternating whitespace / non-whitespace runs (`splitIntoTokensWithAnsi`).
    let mut tokens: Vec<Vec<(&str, Style)>> = Vec::new();
    for cell in cells {
        let ws = is_ws_grapheme(cell.0);
        match tokens.last_mut() {
            Some(tok) if tok.first().is_some_and(|f| is_ws_grapheme(f.0) == ws) => tok.push(cell),
            _ => tokens.push(vec![cell]),
        }
    }

    let mut rows: Vec<Vec<(&str, Style)>> = Vec::new();
    let mut cur: Vec<(&str, Style)> = Vec::new();
    let mut cur_w = 0usize;
    for tok in tokens {
        let tok_w: usize = tok.iter().map(|(g, _)| grapheme_cols(g)).sum();
        let is_ws = tok.first().is_some_and(|(g, _)| is_ws_grapheme(g));
        if tok_w > width && !is_ws {
            if !cur.is_empty() {
                rows.push(std::mem::take(&mut cur));
                cur_w = 0;
            }
            // `breakLongWord` (`:954-1015`) walks the token one GRAPHEME at a time, so a cluster is
            // never split across the break.
            for cell in tok {
                let w = grapheme_cols(cell.0);
                if cur_w + w > width && !cur.is_empty() {
                    rows.push(std::mem::take(&mut cur));
                    cur_w = 0;
                }
                cur_w += w;
                cur.push(cell);
            }
            continue;
        }
        if cur_w + tok_w > width && cur_w > 0 {
            rows.push(std::mem::take(&mut cur));
            cur_w = 0;
            if is_ws {
                continue;
            }
        }
        cur_w += tok_w;
        cur.extend(tok);
    }
    if !cur.is_empty() {
        rows.push(cur);
    }

    rows.into_iter()
        .map(|mut row| {
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
            let mut out = Line::from(spans);
            out.style = line.style;
            out
        })
        .collect()
}

/// Port of `Box.render` (`tui/src/components/box.ts:74-136`), the shell every tinted transcript
/// block is built from.
///
/// Upstream renders its children at `contentWidth = max(1, width - paddingX * 2)` (`:79`), prefixes
/// `leftPad` to every produced row (`:80`, `:87`), then emits `paddingY` background-filled blank
/// rows, the content, and `paddingY` more blank rows (`:106-119`). `applyBg` (`:127-136`) pads each
/// row to the full `width` and paints the background across it. An empty child set renders nothing
/// at all (`:75-77`, `:91-93`) — no stray tinted blanks.
///
/// Callers that pass a non-zero `paddingY` upstream, i.e. every one of them: `tool-execution.ts:68`
/// `new Box(1, 1, toolPendingBg)` (and its `:69` `new Text("", 1, 1, …)` fallback), `edit.ts:158`
/// `new Box(1, 1, …)` (the self-shelled edit block), `user-message.ts:40`
/// `new Box(this.outputPad, 1, userMessageBg)`, `custom-message.ts:36`, `custom-entry.ts:49` and the
/// three `extends Box` message components — `skill-invocation-message.ts:17`,
/// `branch-summary-message.ts:16`, `compaction-summary-message.ts:16` — each `super(1, 1,
/// customMessageBg)`. `git grep "new Box(" v0.84.1 -- packages/` finds no other production site and
/// no caller passing `paddingY = 0`.
pub(super) fn box_lines(
    children: Vec<Line<'static>>,
    width: usize,
    padding_x: usize,
    padding_y: usize,
    bg: Style,
) -> Vec<Line<'static>> {
    if children.is_empty() {
        return Vec::new();
    }
    let content_width = width.saturating_sub(padding_x * 2).max(1);
    let left = " ".repeat(padding_x);
    let mut content: Vec<Line<'static>> = Vec::new();
    for child in &children {
        for mut row in wrap_line(child, content_width) {
            if padding_x > 0 {
                row.spans.insert(0, Span::raw(left.clone()));
            }
            content.push(row);
        }
    }
    if content.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<Line<'static>> = Vec::with_capacity(content.len() + padding_y * 2);
    for _ in 0..padding_y {
        out.push(apply_bg(Line::default(), width, bg));
    }
    for row in content {
        out.push(apply_bg(row, width, bg));
    }
    for _ in 0..padding_y {
        out.push(apply_bg(Line::default(), width, bg));
    }
    out
}

/// `Box.applyBg` (`box.ts:127-136`): right-pad to `width` and paint the background over the whole
/// row.
///
/// The pad is measured with `Line::width()` — ratatui's unicode-width sum, the analogue of
/// upstream's `visibleWidth(line)` (`:128`). It used to be `chars().count()`, which **under**-counts
/// every CJK ideograph, emoji and box-drawing glyph, so the row was over-padded past the frame and
/// spilled into a spurious extra tinted row.
pub(super) fn apply_bg(mut line: Line<'static>, width: usize, bg: Style) -> Line<'static> {
    let vis = line.width();
    if vis < width {
        line.spans.push(Span::raw(" ".repeat(width - vis)));
    }
    line.style = line.style.patch(bg);
    line
}

/// Port of `Text.render` (`tui/src/components/text.ts:60-87`) for one already-styled **[`Line`]**,
/// at `paddingY = 0` — the multi-span form, so a row built from several differently-styled pieces
/// (a `keyHint` pair's dim key + muted description, say) keeps every colour across the wrap.
///
/// Upstream has only this form: a `Text` is constructed from ONE string that already carries its
/// ANSI runs (`theme.fg("dim", …) + theme.fg("muted", …)`), and `wrapTextWithAnsi` carries those
/// runs through the wrap (`utils.ts:770-798`). [`wrap_line`] is that primitive; this adds `:64`'s
/// `contentWidth` and `:70-76`'s left margin. See [`text_lines`] for the single-style convenience
/// wrapper over it.
pub(crate) fn text_lines_of(
    src: &Line<'static>,
    width: usize,
    padding_x: usize,
) -> Vec<Line<'static>> {
    let content_width = width.saturating_sub(padding_x * 2).max(1);
    let left = " ".repeat(padding_x);
    let mut out: Vec<Line<'static>> = Vec::new();
    for mut row in wrap_line(src, content_width) {
        if padding_x > 0 {
            row.spans.insert(0, Span::raw(left.clone()));
        }
        out.push(row);
    }
    out
}

/// Port of `Text.render` (`tui/src/components/text.ts:60-87`) for one already-styled string, at
/// `paddingY = 0`.
///
/// Upstream wraps at `contentWidth = max(1, width - paddingX * 2)` (`:64`) and then emits
/// `leftMargin + line + rightMargin` per produced row (`:70-76`). The right margin and the pad to
/// `width` (`:83-85`) are only observable through a background, and a bare `Text` has none, so they
/// are not materialised here — a trailing run of blanks is invisible in a ratatui cell grid and
/// would only defeat the right-trim [`wrap_line`] performs.
pub(super) fn text_lines(text: &str, width: usize, padding_x: usize, style: Style) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    // `wrapTextWithAnsi` splits on newlines first (`utils.ts:839`) and wraps each piece.
    for logical in text.split('\n') {
        // The style rides on the SPAN, not the `Line` — upstream's colour is baked into the string
        // the `Text` was constructed with (`theme.fg("dim", message)`), inside the margins, so it
        // survives being nested in a `Box` that later paints `Line::style` with a background.
        out.extend(text_lines_of(&Line::from(Span::styled(logical.to_string(), style)), width, padding_x));
    }
    out
}

/// The tool block's shell: `new Box(1, 1, <state bg>)` (tool-execution.ts:68).
pub(super) fn finalize_block(lines: Vec<Line<'static>>, width: usize, bg: Style) -> Vec<Line<'static>> {
    box_lines(lines, width, 1, 1, bg)
}

// --- per-tool renderers ------------------------------------------------------------------------
//
// Each pushes fg-styled logical lines into `out`; `tool_lines` adds the leading spacer + state tint.

/// Port of `replaceTabs` (`core/tools/render-utils.ts:31-33`): `text.replace(/\t/g, "   ")`.
///
/// X6 — every `read`/`write` body row goes through this upstream, on BOTH sides of the
/// `lang ? … : …` ternary (`read.ts:185,190`, `write.ts:153,160`). cyrup pushed the raw string, so a
/// tab expanded at the terminal's own 8-column stops and the block's alignment drifted from Pi's
/// fixed three spaces. Exactly three spaces, not a tab-stop-aware expansion — upstream's regex is
/// positional-context-free.
pub(super) fn replace_tabs(text: &str) -> String {
    text.replace('\t', "   ")
}

/// X6 — one already-`replaceTabs`'d body row, syntax-highlighted when the path resolved to a
/// language and flat `toolOutput` otherwise.
///
/// This is the whole of `read.ts:190`'s ternary
/// `lang ? replaceTabs(line) : theme.fg("toolOutput", replaceTabs(line))` — note the `lang` arm
/// carries NO `toolOutput` colour, because the highlighter already coloured it. Pi highlights the
/// joined body once (`highlightCode(replaceTabs(output), lang)`, `:185`) and splits it back into
/// lines; cyrup's [`crate::markdown::highlight_code_lines`] returns the same per-line vector, so the
/// rows are indexed rather than re-highlighted one at a time (syntect is stateful — highlighting a
/// line in isolation loses an open string/comment run).
pub(super) fn body_line(
    raw: &str,
    highlighted: Option<&Vec<Line<'static>>>,
    idx: usize,
    theme: &UiTheme,
) -> Line<'static> {
    match highlighted.and_then(|h| h.get(idx)) {
        Some(l) => l.clone(),
        None => Line::styled(replace_tabs(raw), theme.tool_output_style()),
    }
}

/// The number of WRAPPED display rows `lines` occupy at `width`, using the **same** word-wrap
/// `ratatui`'s `Paragraph::render` applies with `.wrap(Wrap { trim: false })`. ratatui 0.30's
/// `Paragraph::line_count(width)` runs the identical `WordWrapper` the renderer does, so the measured
/// height EXACTLY matches what render produces — the fix for the PROSE-WRAP truncation: a single
/// long paragraph (one logical [`Line`]) wraps to many display rows, and both the content-sized live
/// viewport ([`crate::transcript::TranscriptView::content_height`]) and the scrollback flush must size to the wrapped count,
/// not the logical line count. `width == 0` degrades to the logical count (nothing renders anyway).
pub(crate) fn wrapped_height(lines: &[Line<'static>], width: usize) -> usize {
    if width == 0 {
        return lines.len();
    }
    Paragraph::new(lines.to_vec())
        .wrap(Wrap { trim: false })
        .line_count(width.min(u16::MAX as usize) as u16)
}

/// Render a single committed [`Entry`] into its styled scrollback line(s) at content `width`
/// (R-ARCH-TUI-003). Used by the shell to feed drained entries into `Terminal::insert_before`.
///
/// Assistant and user bodies render as **markdown** (spec/tui/06 §2) — multiple lines, no role
/// label: `assistant-message.ts:104-114` adds one `Markdown` child per text block and
/// `user-message.ts:38-58` one `Box`-wrapped `Markdown`, and neither contains a prefix (X1).
/// Prefix `leftMargin` to every ALREADY-WRAPPED row — the second half of `markdown.ts:334-340`
/// (`const lineWithMargins = leftMargin + line + rightMargin`), where `leftMargin =
/// " ".repeat(this.paddingX)` (`:329`) and `paddingX` is the caller's `outputPad`
/// (`assistant-message.ts:111` `new Markdown(text, this.outputPad, 0, …)`).
///
/// **Order matters and this is the back half of it.** Upstream wraps at `contentWidth` FIRST
/// (`:322`) and margins SECOND (`:340`); [`crate::markdown::render`] now does that wrap internally
/// (its `width` argument IS `contentWidth` — every call site passes `width - outputPad * 2`), so
/// every row handed here already fits and the indent lands on rows 1..N as well as on row 0. It
/// used to be inserted into the single unwrapped logical line and reflowed afterwards by the outer
/// `Paragraph::wrap` at FULL frame width, which is what put row 0 at column 1 and rows 1..N at
/// column 0 (L2) with no right gutter (M10).
///
/// `rightMargin` (`:330`) and the pad-to-`width` (`:346-348`) are not materialised, for the reason
/// [`text_lines`] gives: without a background they are an invisible trailing run of blanks in a
/// ratatui cell grid, and the gutter they buy comes from the narrower wrap instead. A no-op at
/// `pad == 0` (flush-left).
pub(super) fn pad_lines(lines: &mut [Line<'static>], pad: usize) {
    if pad == 0 {
        return;
    }
    let indent = " ".repeat(pad);
    for line in lines.iter_mut() {
        line.spans.insert(0, Span::raw(indent.clone()));
    }
}
