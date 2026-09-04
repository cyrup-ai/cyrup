use super::*;

impl<'t> MdRenderer<'t> {
    pub(super) fn new(width: usize, theme: &'t UiTheme) -> Self {
        MdRenderer {
            width,
            theme,
            out: Vec::new(),
            cur: Vec::new(),
            prefix_spans: 0,
            bold: 0,
            italic: 0,
            strike: 0,
            heading: None,
            quote: 0,
            lists: Vec::new(),
            items: Vec::new(),
            pending_marker: None,
            link: None,
            link_text: String::new(),
            code_lang: None,
            code_buf: String::new(),
            code_closed: false,
            html_buf: None,
            table: None,
            default_text: None,
            default_italic: false,
            hyperlinks: false,
            mermaid: MermaidContext::OFF,
            math: Vec::new(),
            strike_literal: Vec::new(),
            preserve_list_markers: false,
            preserve_escapes: false,
        }
    }

    /// The base text style with the active inline modifiers applied.
    fn inline_style(&self) -> Style {
        let mut s = if let Some(level) = self.heading {
            // H1 is heading + bold + **underline**; H2–H6 are heading + bold only (Pi markdown.ts:
            // 344-348) — item #9 "markdown H1 underline".
            let base = self.theme.md_heading_style();
            if level == HeadingLevel::H1 {
                base.add_modifier(Modifier::UNDERLINED)
            } else {
                base
            }
        } else if self.link.is_some() {
            self.theme.md_link_style()
        } else if self.quote > 0 {
            self.theme.md_quote_style()
        } else if let Some(c) = self.default_text {
            // Pi's `{ color }` option replaces the *plain prose* foreground only
            // (`markdown.ts:377-404` `applyDefaultStyle`, reached through the inline style context).
            // `{ italic }` is the next line of the same function (`:393-395`) and rides along.
            let base = Style::default().fg(c);
            if self.default_italic {
                base.add_modifier(Modifier::ITALIC)
            } else {
                base
            }
        } else {
            self.theme.assistant_style()
        };
        if self.bold > 0 {
            s = s.add_modifier(Modifier::BOLD);
        }
        if self.italic > 0 {
            s = s.add_modifier(Modifier::ITALIC);
        }
        if self.strike > 0 {
            s = s.add_modifier(Modifier::CROSSED_OUT);
        }
        s
    }

    /// Push owned styled text onto the current line, materializing any pending list marker first.
    ///
    /// **Inside a table cell the destination is the cell, not the row.** Upstream a cell is rendered
    /// by the very same `renderInlineTokens` that renders a paragraph (`markdown.ts:960`, `:983`),
    /// so every inline style the walk is carrying — bold, italic, `code`, link, strikethrough, the
    /// `{ color }`/`{ italic }` default, the blockquote colour — has to reach the cell too (M7).
    /// Routing the capture through here rather than through three separate arms of [`Self::event`]
    /// is what makes that automatic, and it is also what lets a link's ` (url)` suffix
    /// ([`Self::end`], `TagEnd::Link`) land in the cell instead of leaking onto the row.
    pub(super) fn push_text(&mut self, text: &str, style: Style) {
        if self.link.is_some() {
            self.link_text.push_str(text);
        }
        if let Some(table) = self.table.as_mut() {
            table.cur_cell.push(Span::styled(text.to_string(), style));
            return;
        }
        self.open_line();
        self.cur.push(Span::styled(text.to_string(), style));
    }

    /// Open a fresh row: lay down its `firstPrefix` and record how many spans that is.
    ///
    /// A no-op once the row is already open (mid-line `push_text`), which is what keeps
    /// [`Self::prefix_spans`] pointing at the row's own prefix and not at a later span.
    fn open_line(&mut self) {
        if let Some((marker, mstyle)) = self.pending_marker.take() {
            // `firstPrefix = indent + this.theme.listBullet(marker)` (`markdown.ts:774`). The frame
            // records the marker's visible width and flips to `renderedAnyLine` (`:789-791`) so every
            // LATER row of this item pads instead of re-bulleting.
            self.start_line_prefix();
            if let Some(frame) = self.items.last_mut() {
                frame.marker_w = display_width(&marker);
                frame.rendered = true;
            }
            self.cur.push(Span::styled(marker, mstyle));
            // The marker is PART of `firstPrefix` upstream (`:774`), so it counts toward the split
            // point and toward `itemWidth = width - visibleWidth(firstPrefix)` (`:776`).
            self.prefix_spans = self.cur.len();
        } else if self.cur.is_empty() {
            self.start_line_prefix();
        }
    }

    /// Emit the leading quote/indent/list prefix at the start of a fresh line.
    fn start_line_prefix(&mut self) {
        let prefix = self.continuation_prefix();
        self.cur.extend(prefix);
        self.prefix_spans = self.cur.len();
    }

    /// `continuationPrefix` — the prefix every row of a block AFTER its first carries
    /// (`markdown.ts:775`, applied at `:789`).
    ///
    /// Three components, in upstream's order:
    /// 1. `this.theme.quoteBorder("│ ")` once per open blockquote — **visible**, because the quote
    ///    arm re-emits it on every wrapped row (`markdown.ts:594-597`), not just the first.
    /// 2. `const indent = "    ".repeat(depth)` — FOUR spaces per nesting level (`:758`), not two.
    /// 3. `" ".repeat(visibleWidth(marker))` for the INNERMOST rendered item only (`:775`). A nested
    ///    list's rows are pushed by `renderList(…, depth + 1, …)` directly (`:781`) and never go
    ///    through the parent's `linePrefix`, which is exactly what the `items.last()` read
    ///    reproduces.
    ///
    /// Note (1) is a real glyph and (3) is spaces — they are not the same rule, and a wrapped quoted
    /// list depends on both being right.
    ///
    /// This is also verbatim what a FRESH line's prefix is once the item's marker has been emitted,
    /// which is why [`Self::start_line_prefix`] is a thin wrapper over it.
    fn continuation_prefix(&self) -> Vec<Span<'static>> {
        let mut spans: Vec<Span<'static>> = Vec::new();
        for _ in 0..self.quote {
            spans.push(Span::styled(
                "│ ".to_string(),
                self.theme.md_quote_border_style(),
            ));
        }
        let depth = self.lists.len().saturating_sub(1);
        if depth > 0 {
            spans.push(Span::raw("    ".repeat(depth)));
        }
        if let Some(frame) = self.items.last()
            && frame.rendered
            && frame.marker_w > 0
        {
            spans.push(Span::raw(" ".repeat(frame.marker_w)));
        }
        spans
    }

    /// The width a block nested in this container renders at — upstream's `itemWidth`
    /// (`markdown.ts:776` `Math.max(1, width - visibleWidth(firstPrefix))`) and
    /// `quoteContentWidth` (`:568` `Math.max(1, width - 2)`) unified, since cyrup's single event
    /// walk is inside both containers at once where upstream recurses through two `renderToken`
    /// frames.
    ///
    /// A marker still only QUEUED (`pending_marker`) already counts: upstream computes `itemWidth`
    /// from `firstPrefix` before rendering any of the item's children (`:774-776`, `:786`), so the
    /// item's first block is sized past the bullet even though the bullet has not been emitted yet.
    pub(super) fn content_width(&self) -> usize {
        let mut used: usize = usize::try_from(self.quote)
            .unwrap_or(usize::MAX)
            .saturating_mul(2);
        used = used.saturating_add(self.lists.len().saturating_sub(1).saturating_mul(4));
        let marker_w = match (self.pending_marker.as_ref(), self.items.last()) {
            (Some((m, _)), _) => display_width(m),
            (None, Some(frame)) if frame.rendered => frame.marker_w,
            _ => 0,
        };
        used = used.saturating_add(marker_w);
        self.width.saturating_sub(used).max(1)
    }

    /// Flush the current spans as output row(s), wrapping the body to the container's content width
    /// and re-prefixing every produced row (no-op when empty).
    ///
    /// **This is where upstream's wrap happens, and the order is the whole point**: `markdown.ts`
    /// wraps FIRST and prefixes SECOND, in all three of its wrapping sites —
    /// `:322` `for (const wrappedLine of wrapTextWithAnsi(line, contentWidth))` then `:340`
    /// `leftMargin + line + rightMargin`; `:594-597` `wrapTextWithAnsi(styledLine,
    /// quoteContentWidth)` then `quoteBorder("│ ") + wrappedLine`; `:788-791`
    /// `wrapTextWithAnsi(line, itemWidth)` then `linePrefix + wrappedLine`. Wrapping AFTER the
    /// margin has been inserted — which is what an outer `Paragraph::wrap` over `pad_lines`' output
    /// did — leaves row 0 indented and rows 1..N flush at column 0 (L2), and lets the text run into
    /// the last terminal column with no right gutter (M10).
    ///
    /// `wrap_line` returns exactly one (empty) row for an empty body, so a list item that emitted a
    /// marker and nothing else still produces its `firstPrefix` alone —
    /// `if (!renderedAnyLine) lines.push(firstPrefix)` (`markdown.ts:796-798`).
    pub(super) fn flush_line(&mut self) {
        // Guard FIRST. `flush_line` is called from `SoftBreak`, `HardBreak`, `Rule` and every
        // start/end tag boundary; emitting a row for an empty `cur` would put a blank between every
        // pair of blocks and move every spacer count in the transcript's vertical rhythm.
        if self.cur.is_empty() {
            self.prefix_spans = 0;
            return;
        }
        let spans = std::mem::take(&mut self.cur);
        let split = self.prefix_spans.min(spans.len());
        self.prefix_spans = 0;
        let (prefix, body) = spans.split_at(split);
        // Measure what is ACTUALLY on the row, not a recomputed prefix: `visibleWidth(firstPrefix)`
        // (`markdown.ts:776`) is taken from the very string that gets prepended.
        let prefix_w: usize = prefix.iter().map(Span::width).sum();
        // `Math.max(1, width - visibleWidth(firstPrefix))` (`:776`).
        let avail = self.width.saturating_sub(prefix_w).max(1);
        let rows = crate::transcript::wrap_line(&Line::from(body.to_vec()), avail);
        let cont = self.continuation_prefix();
        for (i, row) in rows.into_iter().enumerate() {
            // `const linePrefix = renderedAnyLine ? continuationPrefix : firstPrefix;` (`:789`).
            let mut out_spans: Vec<Span<'static>> = if i == 0 {
                prefix.to_vec()
            } else {
                cont.clone()
            };
            out_spans.extend(row.spans);
            self.out.push(Line::from(out_spans));
        }
    }

    /// Emit an already-built [`Line`] as a block-level row of this container: it collects the same
    /// `firstPrefix`/`continuationPrefix` and the same wrap every prose row does.
    ///
    /// Upstream a block token returns a bare `string[]` from `renderToken` and its CALLER prefixes
    /// it — `this.theme.quoteBorder("│ ") + wrappedLine` inside a blockquote (`markdown.ts:596`),
    /// `linePrefix + wrappedLine` inside a list item (`:790`) — and the top-level post-pass at
    /// `:322` wraps whatever is left. cyrup has no caller to do that, so block emitters route
    /// through here instead of pushing straight onto `self.out`.
    pub(super) fn emit_prefixed(&mut self, line: Line<'static>) {
        self.open_line();
        // `Line::styled` puts the colour on the LINE, not on its spans; the prefix spans carry their
        // own (`mdQuoteBorder`, `mdListBullet`), so the line style has to be folded down onto the
        // body spans or a prefixed row would repaint the bullet in the body colour.
        let lstyle = line.style;
        self.cur.extend(
            line.spans
                .into_iter()
                .map(|s| Span::styled(s.content.into_owned(), lstyle.patch(s.style))),
        );
        self.flush_line();
    }

    /// Push a blank separator row unless the output already ends with one.
    ///
    /// Upstream a separator is the string `""` — `case "space"` (`markdown.ts:619-621`), the
    /// paragraph / heading / code / `hr` trailers (`:484-486`, `:497-499`, `:536-538`, `:607-609`)
    /// and the loose-list gap (`:800`). Whether it stays bare is decided by the CONTAINER that
    /// receives it, and the two containers do **not** answer the same way:
    ///
    /// * **A blockquote materialises it.** The separator is just another entry of
    ///   `renderedQuoteLines`, and `:592-598` prepends `quoteBorder("│ ")` to every entry it walks —
    ///   blanks included — so the border runs unbroken down the block. Pushing a bare
    ///   `Line::default()` there punched a hole in it.
    /// * **A list does not, observably.** The loose-list gap at `:800` is pushed by `renderList`
    ///   straight into its own `lines`, never through `linePrefix`, so it really is bare; a gap
    ///   *inside* an item goes through `:786-793` instead and comes out as `continuationPrefix + ""`,
    ///   i.e. a run of trailing SPACES. Both paint an empty terminal row, so only the quote border —
    ///   the one prefix with a glyph in it — is materialised here.
    pub(super) fn blank(&mut self) {
        if self
            .out
            .last()
            .map(|l| row_is_blank(l, self.quote))
            .unwrap_or(true)
        {
            return;
        }
        let mut spans: Vec<Span<'static>> = Vec::new();
        for _ in 0..self.quote {
            spans.push(Span::styled(
                "│ ".to_string(),
                self.theme.md_quote_border_style(),
            ));
        }
        self.out.push(if spans.is_empty() {
            Line::default()
        } else {
            Line::from(spans)
        });
    }

    /// Emit literal source text through the same three-way sink as [`Event::Text`] (table cell /
    /// code buffer / styled inline run). Used for the tildes of a non-strikethrough `~…~` run.
    fn emit_literal(&mut self, text: &str) {
        if self.code_lang.is_some() {
            self.code_buf.push_str(text);
        } else {
            let style = self.inline_style();
            self.push_text(text, style);
        }
    }

    /// Push a text run, expanding any `\u{f0006}<index>\u{f0007}` math placeholder
    /// [`latex_prepass`] left in it (M12).
    ///
    /// A rendered expression may be several rows tall — a stacked fraction, a limit operator, a
    /// matrix — and `case "latexBlock"` pushes each on its own line (`markdown.ts:511-513`), so a
    /// row break here is a real row break. Inside a table cell there is no row to break, so the
    /// rows are joined with a space instead; upstream never reaches that case because `renderTable`
    /// wraps the cell afterwards anyway.
    fn emit_with_math(&mut self, text: &str, style: Style) {
        if !text.contains(MATH_START) {
            self.push_text(text, style);
            return;
        }
        let mut buf = String::new();
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            if c != MATH_START {
                buf.push(c);
                continue;
            }
            let mut digits = String::new();
            while chars.peek().is_some_and(char::is_ascii_digit) {
                if let Some(d) = chars.next() {
                    digits.push(d);
                }
            }
            if chars.peek() == Some(&MATH_END) {
                chars.next();
            }
            let rows = digits
                .parse::<usize>()
                .ok()
                .and_then(|i| self.math.get(i))
                .cloned();
            let Some(rows) = rows else { continue };
            for (i, row) in rows.iter().enumerate() {
                if i > 0 {
                    if self.table.is_some() {
                        buf.push(' ');
                    } else {
                        if !buf.is_empty() {
                            self.push_text(&buf, style);
                            buf.clear();
                        }
                        self.flush_line();
                    }
                }
                buf.push_str(row);
            }
        }
        if !buf.is_empty() {
            self.push_text(&buf, style);
        }
    }

    /// `escaped` is set only for an `Event::Text` run that the source opened with a backslash
    /// escape, and only while [`MdRenderer::preserve_escapes`] is on — see the call site in
    /// `render_inner`, which is where the backslash is recovered from.
    pub(super) fn event(&mut self, ev: Event<'_>, raw: &str, escaped: bool) {
        match ev {
            Event::Start(tag) => self.start(tag, raw),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => {
                if self.code_lang.is_some() {
                    // Inside a fence the backslash was never an escape — pulldown-cmark hands the
                    // fence body over verbatim — so there is nothing to put back.
                    self.code_buf.push_str(&t);
                } else {
                    let style = self.inline_style();
                    if escaped {
                        // `preserveBackslashEscapes ? token.raw : token.text` (`markdown.ts:656`):
                        // exactly one backslash per escape, and pulldown-cmark starts a fresh text
                        // run at every escaped character, so one per run is the whole of `raw`.
                        self.push_text("\\", style);
                    }
                    self.emit_with_math(&t, style);
                }
            }
            Event::Code(c) => {
                // `case "codespan": result += this.theme.code(token.text) + stylePrefix`
                // (`markdown.ts:685-687`) — inside a table cell exactly as inside a paragraph (M7).
                let style = self.theme.md_code_style();
                self.push_text(&c, style);
            }
            Event::SoftBreak => {
                // A source line break inside a paragraph stays a line break: marked keeps the `\n`
                // inside the text token (which is why `renderInlineTokens` splits and rejoins on
                // `\n`, `markdown.ts:638-641`) and `wrapTextWithAnsi` then splits the rendered line
                // on `/\r\n|\r|\n/` into one output row per source line (`utils.ts:839`). It is NOT
                // collapsed to a space.
                if self.table.is_none() {
                    self.flush_line();
                }
            }
            Event::HardBreak => self.flush_line(),
            Event::Rule => {
                self.flush_line();
                // `lines.push(this.theme.hr("─".repeat(Math.min(width, 80))))` (`markdown.ts:606`)
                // — `width` there is whatever `renderToken` was handed, i.e. `itemWidth` inside a
                // list item (`:786`) and `quoteContentWidth` inside a blockquote (`:583`), not the
                // component width. Then `:790`/`:596` prefixes the row.
                let n = self.content_width().clamp(1, 80);
                self.emit_prefixed(Line::styled("─".repeat(n), self.theme.md_hr_style()));
                // `case "hr": … if (nextTokenType && nextTokenType !== "space") lines.push("")`
                // (`markdown.ts:605-610`); when a `space` token *does* follow, `:619-622` supplies
                // the blank instead. Either way exactly one — which is what `blank()` guarantees.
                self.blank();
            }
            Event::TaskListMarker(checked) => {
                let mark = if checked { "[x] " } else { "[ ] " };
                // `marker = bullet + taskMarker` (`markdown.ts:770-773`): the task box is APPENDED to
                // the `- ` bullet the item already queued, not a replacement for it, and the whole
                // marker carries `listBullet` (`:774`).
                match self.pending_marker.as_mut() {
                    Some((marker, _)) => marker.push_str(mark),
                    None => {
                        self.pending_marker =
                            Some((mark.to_string(), self.theme.md_list_bullet_style()));
                    }
                }
            }
            // INLINE html concatenates into the surrounding run: `result +=
            // applyTextWithNewlines(token.raw)` (`markdown.ts:721-726`). Unchanged.
            Event::InlineHtml(h) => {
                let style = self.inline_style();
                self.push_text(h.trim_end_matches('\n'), style);
            }
            // BLOCK html is one token upstream (`markdown.ts:612-617`) but one event per source
            // line here, so accumulate and let `TagEnd::HtmlBlock` do the trim-and-split. The
            // `None` leg is the belt-and-braces path for an `Event::Html` that arrives outside a
            // `Tag::HtmlBlock` span: it keeps today's inline-style behaviour rather than dropping
            // the text.
            Event::Html(h) => match self.html_buf.as_mut() {
                Some(buf) => buf.push_str(&h),
                None => {
                    let style = self.inline_style();
                    self.push_text(h.trim_end_matches('\n'), style);
                }
            },
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>, raw: &str) {
        match tag {
            Tag::Heading { level, .. } => {
                self.flush_line();
                self.heading = Some(level);
                if matches!(
                    level,
                    HeadingLevel::H3 | HeadingLevel::H4 | HeadingLevel::H5 | HeadingLevel::H6
                ) {
                    let hashes = "#".repeat(heading_depth(level));
                    let style = self.theme.md_heading_style();
                    self.push_text(&format!("{hashes} "), style);
                }
            }
            Tag::Paragraph => self.flush_line(),
            // A block-level `html` token is a block: it closes whatever row was open, exactly as
            // `case "paragraph"` does, and then captures until `TagEnd::HtmlBlock`.
            Tag::HtmlBlock => {
                self.flush_line();
                self.html_buf = Some(String::new());
            }
            Tag::List(start) => {
                self.flush_line();
                self.lists.push(start);
            }
            Tag::Item => {
                // `preserveOrderedListMarkers` (`markdown.ts:765-771`) replaces BOTH synthesized
                // bullets with the item's own source marker, `??`-falling back to the synthesized
                // one when the pattern does not match — which is what marked's `item.raw` is for
                // there. `Start(Item)`'s offset range is that same slice: it opens AT the marker,
                // with any enclosing blockquote's `> ` and any nesting indent already outside it
                // (unlike `Start(Table)`'s raw, which is why that one needs
                // [`strip_quote_markers`]), so pi's two patterns apply to it unchanged.
                let preserve = self.preserve_list_markers;
                let marker = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        let s = format!("{n}. ");
                        // The counter advances on BOTH paths: a preserved `1)` must not stop the
                        // next item — which may fall back — from being numbered as if it had
                        // counted (upstream numbers off `startNumber + i`, `markdown.ts:766`).
                        *n = n.saturating_add(1);
                        match preserve.then(|| source_ordered_marker(raw)).flatten() {
                            Some(literal) => literal,
                            None => s,
                        }
                    }
                    _ => match preserve.then(|| source_unordered_marker(raw)).flatten() {
                        Some(literal) => literal,
                        None => "- ".to_string(),
                    },
                };
                // A fresh `renderedAnyLine = false` per item (`markdown.ts:777`).
                self.items.push(ItemFrame::default());
                self.pending_marker = Some((marker, self.theme.md_list_bullet_style()));
            }
            Tag::BlockQuote(_) => {
                self.flush_line();
                self.quote = self.quote.saturating_add(1);
            }
            Tag::CodeBlock(kind) => {
                self.flush_line();
                self.code_lang = Some(match kind {
                    // The WHOLE (trimmed) info string, not just its first word: marked sets
                    // `token.lang` to the trimmed info string — which is why every consumer that
                    // wants the bare language splits it itself, e.g. `mermaid.ts:15`
                    // `token.lang?.trim().split(/\s+/, 1)[0]?.toLowerCase() === "mermaid"`. Pi's
                    // fence line is `` `${"```"}${token.lang || ""}` `` (`markdown.ts:522`) and its
                    // highlighter is handed the same unsplit string (`:524` →
                    // `theme.ts:1268-1272` `supportsLanguage(lang)`), so `js title="x"` prints in
                    // full AND falls back to a flat body — which is exactly what
                    // `highlight_lines`'s `find_syntax_by_token` does with it here.
                    pulldown_cmark::CodeBlockKind::Fenced(info) => info.trim().to_string(),
                    pulldown_cmark::CodeBlockKind::Indented => String::new(),
                });
                // The fence's CLOSED-ness, read off its own source while that source is in hand:
                // `raw` is this event's range, and a `Start` tag's range spans the whole element
                // (the same property `Tag::Table` below relies on for its `token.raw` fallback), so
                // it runs from the opening delimiter through the closing one. A closed fence's body
                // can never change again, which is what makes it memoisable (PERF-005 §3.0b).
                self.code_closed = fence_is_closed(raw);
                self.code_buf.clear();
            }
            Tag::Emphasis => self.italic = self.italic.saturating_add(1),
            Tag::Strong => self.bold = self.bold.saturating_add(1),
            Tag::Strikethrough => {
                // Pi installs a `StrictStrikethroughTokenizer` whose `del()` only matches
                // `/^(~~)(?=[^\s~])…\1(?=[^~]|$)/` (`markdown.ts:7-24`, `:171-174`), so a
                // SINGLE-tilde run is never a `del` token — `~/path~` and `a~b~c` keep their tildes
                // and their normal styling. pulldown-cmark's GFM strikethrough accepts both `~` and
                // `~~`, so reject the single-tilde form here from the source delimiter.
                let literal = !raw.starts_with("~~");
                self.strike_literal.push(literal);
                if literal {
                    self.emit_literal("~");
                } else {
                    self.strike = self.strike.saturating_add(1);
                }
            }
            Tag::Link { dest_url, .. } => {
                self.link = Some(dest_url.to_string());
                self.link_text.clear();
            }
            Tag::Table(_) => {
                self.flush_line();
                // `token.raw` — the fallback body when the pane is too narrow for the grid
                // (`markdown.ts:854-861`). The offset iterator's `Start(Table)` range is the whole
                // table source (header + delimiter row + body).
                self.table = Some(TableCapture {
                    raw: raw.to_string(),
                    ..TableCapture::default()
                });
            }
            Tag::TableHead => {
                if let Some(t) = self.table.as_mut() {
                    t.in_head = true;
                }
            }
            Tag::TableRow => {
                if let Some(t) = self.table.as_mut() {
                    t.cur_row.clear();
                }
            }
            Tag::TableCell => {
                if let Some(t) = self.table.as_mut() {
                    t.cur_cell.clear();
                }
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Heading(_) => {
                self.flush_line();
                self.heading = None;
                self.blank();
            }
            TagEnd::Paragraph => {
                self.flush_line();
                self.blank();
            }
            TagEnd::HtmlBlock => self.emit_html_block(),
            TagEnd::List(_) => {
                self.lists.pop();
                if self.lists.is_empty() {
                    self.blank();
                }
            }
            TagEnd::Item => {
                // `if (!renderedAnyLine) { lines.push(firstPrefix); }` (`markdown.ts:796-798`) — an
                // item whose children produced no row STILL emits its `firstPrefix`, alone, on a row
                // of its own, so `- \n- x` is two rows and not one. [`Self::flush_line`]'s
                // empty-`cur` guard fires before `pending_marker` is ever materialised, so nothing
                // was emitted AND the marker survived into the next `Start(Item)`, which overwrote
                // it — the bullet vanished from the render entirely.
                //
                // The condition is upstream's `renderedAnyLine`, not "the item had children": an
                // item holding only a NESTED list renders no row of its own here either, and
                // upstream sets `renderedAnyLine = true` for it at `:779-783` before `continue`, so
                // its own bullet is legitimately dropped. That case is excluded by
                // `pending_marker.is_some()` — the nested `Start(Item)` has already taken it.
                if self.pending_marker.is_some() && self.items.last().is_some_and(|f| !f.rendered) {
                    self.open_line();
                }
                self.flush_line();
                self.items.pop();
                // An item that emitted nothing still leaves no marker queued for the NEXT item.
                self.pending_marker = None;
            }
            TagEnd::BlockQuote(_) => {
                self.flush_line();
                // "Avoid rendering an extra empty quote line before the outer blockquote spacing" —
                // `while (renderedQuoteLines[len - 1] === "") renderedQuoteLines.pop()`
                // (`markdown.ts:587-590`). The separator this quote's last block queued never
                // reaches `:592-598`, so the block cannot end on a dangling `│ `; the single blank
                // that follows a blockquote is `:599-601`'s BARE `""`, which `blank()` supplies once
                // the depth is back to zero.
                while self
                    .out
                    .last()
                    .is_some_and(|l| is_quote_only_row(l, self.quote))
                {
                    self.out.pop();
                }
                self.quote = self.quote.saturating_sub(1);
                if self.quote == 0 {
                    self.blank();
                }
            }
            TagEnd::CodeBlock => {
                let lang = self.code_lang.take().unwrap_or_default();
                let code = std::mem::take(&mut self.code_buf);
                let closed = std::mem::take(&mut self.code_closed);
                if self.mermaid_applies(&lang) {
                    self.emit_mermaid_block(&lang, &code, closed);
                } else {
                    self.emit_code_block(&lang, &code, closed);
                }
            }
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Strikethrough => match self.strike_literal.pop() {
                // A single-tilde run Pi never tokenizes as `del`: close it with its literal `~`.
                Some(true) => self.emit_literal("~"),
                _ => self.strike = self.strike.saturating_sub(1),
            },
            TagEnd::Link => {
                if let Some(href) = self.link.take() {
                    let text = std::mem::take(&mut self.link_text);
                    let stripped = href.strip_prefix("mailto:").unwrap_or(&href);
                    // `if (getCapabilities().hyperlinks) { result += hyperlink(styledLink,
                    // token.href) … }` — on a hyperlink-capable terminal the URL is NOT printed
                    // inline, "regardless of whether it matches href" (`markdown.ts:692-696`). The
                    // ` (url)` suffix is the incapable-terminal fallback only (`:697-707`).
                    // The fallback test is EXACTLY `token.text === token.href || token.text ===
                    // hrefForComparison` (`markdown.ts:701-702`) — there is no emptiness clause. An
                    // empty-texted link `[](https://x)` is `"" !== href`, so upstream DOES print the
                    // ` (url)` suffix; suppressing it here swallowed the only trace of the link.
                    // BOTH disjuncts: `token.text === token.href || token.text ===
                    // hrefForComparison` (`markdown.ts:701-702`). Testing only the stripped form
                    // misses a link whose text is the FULL `mailto:` href — `[mailto:a@b](mailto:a@b)`
                    // — which upstream treats as self-describing and cyrup would have followed with
                    // a redundant ` (mailto:a@b)`.
                    if !self.hyperlinks && text != href && text != stripped {
                        let style = self.theme.md_link_url_style();
                        self.push_text(&format!(" ({href})"), style);
                    }
                }
            }
            TagEnd::Table => self.emit_table(),
            TagEnd::TableCell => {
                if let Some(t) = self.table.as_mut() {
                    let cell = std::mem::take(&mut t.cur_cell);
                    t.cur_row.push(trim_cell(cell));
                }
            }
            TagEnd::TableRow => {
                if let Some(t) = self.table.as_mut() {
                    let row = std::mem::take(&mut t.cur_row);
                    if t.in_head {
                        t.header = row;
                    } else {
                        t.rows.push(row);
                    }
                }
            }
            TagEnd::TableHead => {
                if let Some(t) = self.table.as_mut() {
                    // pulldown-cmark emits the header cells as direct children of `TableHead` (no
                    // enclosing `TableRow`), so flush the accumulated header cells here.
                    if !t.cur_row.is_empty() {
                        t.header = std::mem::take(&mut t.cur_row);
                    }
                    t.in_head = false;
                }
            }
            _ => {}
        }
    }

    /// Emit the raw HTML captured since `Tag::HtmlBlock`, one row per source line.
    ///
    /// `case "html"` (`markdown.ts:612-617`) pushes `this.applyDefaultStyle(token.raw.trim())` as a
    /// SINGLE entry that still contains its newlines, and `render()` then hands every entry to
    /// `wrapTextWithAnsi(line, contentWidth)` (`:322`), whose first act is
    /// `text.split(/\r\n|\r|\n/)` (`utils.ts:832-839`). So an N-line `<details>…</details>` prints
    /// as N rows in source order. cyrup's [`crate::transcript::wrap_line`] has no newline split —
    /// it is only ever handed already-split rows — so the split happens here instead, and the rows
    /// leave through [`Self::push_text`] + [`Self::flush_line`] so the list-item / blockquote
    /// prefix machinery still lays `firstPrefix` on row 1 and `continuationPrefix` on rows 2..N.
    ///
    /// The `trim()` is upstream's and is defined on the WHOLE block, which is what guarantees no
    /// leading or trailing blank row while an interior blank line survives as its own row (hence
    /// `split`, not `lines()`).
    ///
    /// **[CYRUP-DELTA]** upstream's `case "html"` pushes no trailing `""` spacer of its own — the
    /// blank after the block comes from the following `space` token (`markdown.ts:619-622`).
    /// pulldown-cmark has no `space` event, so the separator is supplied here instead;
    /// [`Self::blank`] is idempotent against an already-blank last row, so the observable output
    /// matches pi's even though the token stream does not.
    fn emit_html_block(&mut self) {
        let Some(buf) = self.html_buf.take() else {
            return;
        };
        // `\r\n` first so the split below cannot manufacture an empty segment from the pair.
        let text = buf.replace("\r\n", "\n");
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let style = self.inline_style();
        for line in text.split(['\n', '\r']) {
            // An interior blank line is a row of its own upstream (`utils.ts:839` yields an empty
            // segment and `wrapTextWithAnsi` returns `[""]` for it), so the empty case is pushed
            // too rather than skipped: the zero-width span it leaves in `cur` is what stops
            // [`Self::flush_line`]'s empty-`cur` guard from swallowing the row.
            self.push_text(line, style);
            self.flush_line();
        }
        self.blank();
    }

    /// Whether this fence takes the mermaid path (`mermaid.ts:63-74`).
    ///
    /// The `lists`/`quote` guard is not decoration: pi's transformer walks
    /// `markdownParser.lexer(markdown)` (`mermaid.ts:71-72`), which yields the TOP-LEVEL token list
    /// only, so a mermaid fence nested inside a list item or a blockquote is never a `code` token
    /// it sees and stays a raw fence upstream. It is also what makes [`MdRenderer::width`] the
    /// exact `context.availableWidth` pi compares the diagram against (`markdown.ts:284`) rather
    /// than a container-narrowed value.
    fn mermaid_applies(&self, lang: &str) -> bool {
        self.mermaid.enabled()
            && self.lists.is_empty()
            && self.quote == 0
            && mermaid::is_mermaid(lang)
    }

    /// Emit a top-level ```` ```mermaid ```` fence as a Unicode diagram, or fall back the way pi
    /// does (`mermaid.ts:74-85`).
    ///
    /// Every row leaves through [`Self::emit_prefixed`] — the module doc's non-negotiable rule — so
    /// the second wrap at `self.width` applies. [`crate::transcript::wrap_line`] returns a verbatim
    /// clone for a row that already fits, so the box-drawing and the leading spaces survive intact;
    /// that is the property pi's `codeSpan` re-encoding (`:18-36`) had to fake through a markdown
    /// round-trip and which this renderer gets for free.
    ///
    /// Each row is themed per span class, `themedLines`/`styleSpan` (`mermaid.ts:38-57`): six
    /// classes, one [`Span`] per run. The classes are not reported by the engine — they are
    /// inferred from the rendered geometry by the private `mermaid::classify`, whose module doc
    /// names the cases where that inference is wrong.
    fn emit_mermaid_block(&mut self, lang: &str, code: &str, closed: bool) {
        match mermaid::render_diagram(code, self.width, self.mermaid.is_streaming) {
            DiagramOutcome::Diagram(rows) => {
                for row in rows {
                    let spans: Vec<Span<'static>> = row
                        .into_iter()
                        .map(|(class, text)| Span::styled(text, self.span_class_style(class)))
                        .collect();
                    self.emit_prefixed(Line::from(spans));
                }
                self.blank();
            }
            // `return token.raw` — the fence exactly as it would have rendered untransformed.
            DiagramOutcome::Raw => self.emit_code_block(lang, code, closed),
            // `` `${token.raw}\n${codeSpan(styledWarning)}  \n` `` (`:81`): the untouched fence,
            // then the warning line with no blank between them — hence the `emit_fence_rows` split.
            DiagramOutcome::Warned(msg) => {
                self.emit_fence_rows(lang, code, closed);
                self.emit_prefixed(Line::styled(msg, self.theme.warning_style()));
                self.blank();
            }
        }
    }

    /// `styleSpan` (`mermaid.ts:38-53`), role for role.
    ///
    /// Every role already exists on [`UiTheme`]; `text` is [`UiTheme::custom_message_text_style`]
    /// because that is the accessor cyrup already maps pi's `"text"` role to (see the
    /// `RenderTheme::fg` table in [`crate::theme`]) — pi aliases `customMessageText` to the `text`
    /// palette entry (`theme/dark.json:42`), so the two are one colour upstream as well.
    fn span_class_style(&self, class: SpanClass) -> Style {
        match class {
            SpanClass::Border => self.theme.border_muted_style(),
            SpanClass::Text => self.theme.custom_message_text_style(),
            SpanClass::Edge => self.theme.accent_style(),
            SpanClass::EdgeLabel => self.theme.muted_style(),
            // `theme.fg("accent", theme.bold(span.text))` (`:48-49`).
            SpanClass::Title => self.theme.accent_style().add_modifier(Modifier::BOLD),
            // `return span.text` (`:50-51`) — unstyled.
            SpanClass::None => Style::default(),
        }
    }

    /// Emit a fenced code block: top fence line, highlighted (or flat) body, bottom fence line, and
    /// the separating blank that follows any block.
    fn emit_code_block(&mut self, lang: &str, code: &str, closed: bool) {
        self.emit_fence_rows(lang, code, closed);
        self.blank();
    }

    /// [`Self::emit_code_block`] without the trailing blank — the fence rows alone, which is what
    /// pi's `token.raw` is when the warning line has to follow it immediately (`mermaid.ts:81`).
    fn emit_fence_rows(&mut self, lang: &str, code: &str, closed: bool) {
        // marked's `fences` tokenizer is
        // `/^ {0,3}(`{3,}(?=[^`\n]*\n)|~{3,})([^\n]*)(?:\n|$)(?:|([\s\S]*?)(?:\n|$))(?: {0,3}\1[~`]* *(?=\n|$)|$)/`
        // — the body is capture 3 and the `(?:\n|$)` that follows it consumes the newline BEFORE the
        // closing fence, so that newline is not in `token.text`. Pi then does
        // `token.text.split("\n")` (`markdown.ts:530`, and `highlightCode(token.text, …)` on the
        // highlighted path at `:524`), which yields ONE line for a one-line body.
        //
        // pulldown-cmark's code-block `Text` events DO include that final newline, so splitting the
        // buffer as-is yields a trailing `""` and every fenced block grew a spurious indent-only row
        // between the last code line and the closing fence. Strip exactly one trailing `\n` — not
        // `trim_end`, which would also eat a deliberately blank final code line.
        let code = code.strip_suffix('\n').unwrap_or(code);
        let border = self.theme.md_code_block_border_style();
        // Routed through [`Self::emit_prefixed`] rather than straight onto `self.out`: upstream a
        // `code` token returns a bare `string[]` (`markdown.ts:520-540`) whose caller prefixes it —
        // `linePrefix + wrappedLine` inside a list item (`:790`) — and whose long rows the top-level
        // post-pass wraps at `contentWidth` (`:322`). A fence inside a `- ` item therefore lines up
        // under the item's text, and an over-wide code row breaks instead of running off the pane.
        //
        // The `  ` code indent (`:521` `codeBlockIndent ?? "  "`) stays in the BODY, not the prefix,
        // which is what makes a wrapped code row lose it upstream: `wrapSingleLine` never starts a
        // produced row with whitespace (`utils.ts:912-915`).
        self.emit_prefixed(Line::styled(format!("```{lang}"), border));
        // `closed` is this block's memo-eligibility: a fence that carries its closing
        // delimiter is frozen, so it belongs in the highlighter's memo, and only the still-growing
        // tail may claim the single resumable cursor (PERF-005 §3.0b rule (b)).
        for line in highlight_lines(code, lang, self.theme, closed) {
            self.emit_prefixed(line);
        }
        self.emit_prefixed(Line::styled("```".to_string(), border));
    }

    pub(super) fn finish(mut self) -> Vec<Line<'static>> {
        self.flush_line();
        // **Upstream wraps TWICE.** `renderList` (`:788`) and the blockquote arm (`:594`) wrap a
        // child at the CONTAINER width and then prefix it; `render()` afterwards runs every line the
        // token walk produced through `wrapTextWithAnsi(line, contentWidth)` one more time
        // (`markdown.ts:316-326`), before the margins go on at `:328-340`.
        //
        // The second pass is a no-op for every row that already fits — [`crate::transcript::wrap_line`]
        // returns a verbatim clone then, so span structure, styles and trailing spaces all survive.
        // It exists for the rows the inner wrap cannot bound: `avail` floors at
        // `max(1, width - prefix_w)` (`:776` `Math.max(1, …)`), so once the accumulated
        // `│ `/indent/marker prefix is as wide as the pane, `prefix + body` overruns `self.width`.
        // Deeply nested quoted lists at a narrow pane are exactly that case.
        let width = self.width;
        self.out = std::mem::take(&mut self.out)
            .into_iter()
            .flat_map(|line| crate::transcript::wrap_line(&line, width))
            .collect();
        // Drop a single trailing blank line for tight scrollback packing.
        if self
            .out
            .last()
            .map(|l| l.spans.iter().all(|s| s.content.trim().is_empty()))
            .unwrap_or(false)
        {
            self.out.pop();
        }
        self.out
    }
}

/// The `(?: {0,3})` prefix both source-marker patterns open with (`markdown.ts:744`, `:749`):
/// CommonMark's up-to-three-space indent, no more. A fourth space is indented content, and the
/// pattern is anchored, so it simply does not match — which is the `??` fallback's cue.
fn strip_indent(raw: &str) -> &str {
    let mut s = raw;
    for _ in 0..3 {
        match s.strip_prefix(' ') {
            Some(rest) => s = rest,
            None => break,
        }
    }
    s
}

/// `getOrderedListMarker` (`markdown.ts:743-746`): `/^(?: {0,3})(\d{1,9}[.)])[ \t]+/`, returning
/// `` `${match[1]} ` `` — the source digits and their `.`/`)` delimiter, plus exactly one space.
///
/// Hand-ported rather than pulling in `regex`, and byte-wise via `strip_prefix` so no `str` index
/// or slice is taken (`deny(clippy::string_slice)`, `deny(clippy::indexing_slicing)`).
fn source_ordered_marker(raw: &str) -> Option<String> {
    let s = strip_indent(raw);
    // `\d{1,9}` — a tenth digit leaves a digit where `[.)]` must be, so the whole pattern fails.
    let digits: String = s
        .bytes()
        .take(9)
        .take_while(u8::is_ascii_digit)
        .map(char::from)
        .collect();
    if digits.is_empty() {
        return None;
    }
    let rest = s.strip_prefix(digits.as_str())?;
    let (delim, rest) = match rest.strip_prefix('.') {
        Some(rest) => ('.', rest),
        None => (')', rest.strip_prefix(')')?),
    };
    // `[ \t]+` — at least one.
    if !rest.starts_with([' ', '\t']) {
        return None;
    }
    Some(format!("{digits}{delim} "))
}

/// `getUnorderedListMarker` (`markdown.ts:748-751`):
/// `/^(?: {0,3})([-+*])(?:[ \t]+|(?=\r?\n|$))/`, returning `` `${match[1]} ` `` — the source
/// `-`/`+`/`*`, plus exactly one space. The lookahead leg is the empty item (`-` alone on its
/// line), which carries no space to consume.
fn source_unordered_marker(raw: &str) -> Option<String> {
    let s = strip_indent(raw);
    let (bullet, rest) = ['-', '+', '*']
        .into_iter()
        .find_map(|b| Some((b, s.strip_prefix(b)?)))?;
    let ends_line = rest.is_empty() || rest.starts_with('\n') || rest.starts_with("\r\n");
    if rest.starts_with([' ', '\t']) || ends_line {
        Some(format!("{bullet} "))
    } else {
        None
    }
}
