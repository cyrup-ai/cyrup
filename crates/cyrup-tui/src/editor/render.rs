use super::*;

impl InputEditor {
    /// The hardware-cursor position inside `area` (for IME placement, R-10-009), if focused. Accounts
    /// for the top border rule drawn by [`Component::render`]; the editor has no side borders.
    pub fn cursor_in(&self, area: Rect) -> Option<(u16, u16)> {
        if !self.focused {
            return None;
        }
        // Map the logical caret `(row, col)` to its VISUAL `(vrow, vcol)` via the wrap map (built at
        // the render `view_width`) so the hardware cursor lands on the wrapped row/column, matching
        // the reverse-video soft-cursor cell drawn in `render` (Pi `editor.ts:545-551`).
        let map = self.visual_line_map();
        let vi = self.current_visual_line(&map);
        let vl = map.get(vi).copied().unwrap_or(VisualLine { logical: 0, start: 0, len: 0 });
        let vcol = self.col.saturating_sub(vl.start);
        // The caret's COLUMN is the visible width of the text before it, not its char count. Pi never
        // does this arithmetic — it emits a zero-width `CURSOR_MARKER` *inside* the row string
        // (`editor.ts:550`) and lets the terminal advance the cursor by the real cell widths — so a
        // char-count offset is a cyrup-only bug: one emoji ahead of the caret and the hardware cursor
        // (and hence the IME candidate window) lands a column left of the reverse-video cell.
        let before_width: usize = self
            .lines
            .get(vl.logical)
            .map(|l| {
                let s: String = l.iter().skip(vl.start).take(vcol).collect();
                Span::raw(s).width()
            })
            .unwrap_or(0);
        // The text rows start `editorPaddingX` columns in (`Padding::horizontal` on the render
        // block), so the caret must too — Pi prefixes the same `leftPadding` (`editor.ts:522`).
        let x = area
            .x
            .saturating_add(self.effective_padding(area.width))
            .saturating_add(before_width.min(u16::MAX as usize) as u16);
        // The caret rides the SCROLLED window: row `vi` is drawn at `vi - scrollOffset` inside the
        // rules (`editor.ts:519` slices `layoutLines` from `scrollOffset`).
        let y = area
            .y
            .saturating_add(1)
            .saturating_add(vi.saturating_sub(self.scroll_offset).min(u16::MAX as usize) as u16);
        let max_x = area.x.saturating_add(area.width).saturating_sub(1);
        let max_y = area.y.saturating_add(area.height).saturating_sub(1);
        Some((x.min(max_x), y.min(max_y)))
    }
}

/// Split one visual line's `seg_len` chars into styled zones (CMDHINT_01).
///
/// `token` is a char range in the LOGICAL line; `vl` gives the window this visual line slices out of
/// it. Returns at most three `(start_in_seg, len, style)` zones — `base` head, `accent` token, `base`
/// tail — covering `0..seg_len` contiguously and left-to-right; `None` slots are absent zones. Only
/// the visual line(s) overlapping the token produce a non-trivial split — every other line gets one
/// `base` zone, exactly what the code did before. This is the one genuinely new geometry case: a
/// long command name wrapped by `word_wrap_line` across two visual lines must stay highlighted on
/// both. `word_wrap_line` returns `(start, len)` pairs that tile the logical line exactly (`:2296`
/// pushes the final `(chunk_start, n - chunk_start)`; `visual_line_map:551-552` converts them), so
/// every char lands in exactly one window and the intersection below is total.
///
/// The HEAD zone is unreachable under today's invariant and kept deliberately: `token.start` is
/// always 0 (the `/` is char 0 of line 0), so `lo == win_start` and `a == 0` on every window. Keeping
/// the slot makes this a total function of an arbitrary contiguous range rather than one that
/// silently assumes a zero start — do not "simplify" it away, and do not be alarmed when manual
/// testing never exercises it.
///
/// A fixed ARRAY, not a `Vec`: called once per VISIBLE visual line on every frame (~20/frame), and
/// the ≤ 3 bound is structural, so the heap allocation buys nothing. See the perf section.
pub(super) fn style_zones(
    vl: &VisualLine,
    seg_len: usize,
    token: Option<&std::ops::Range<usize>>,
    base: Style,
    accent: Style,
) -> [Option<(usize, usize, Style)>; 3] {
    let plain = [Some((0usize, seg_len, base)), None, None];
    // Only logical line 0 ever carries a command token.
    let Some(tok) = token.filter(|_| vl.logical == 0) else { return plain };
    let win_start = vl.start;
    let win_end = win_start.saturating_add(seg_len);
    let lo = tok.start.max(win_start);
    let hi = tok.end.min(win_end);
    if lo >= hi {
        return plain;
    }
    let (a, b) = (lo.saturating_sub(win_start), hi.saturating_sub(win_start));
    [
        (a > 0).then_some((0, a, base)),
        Some((a, b.saturating_sub(a), accent)),
        (b < seg_len).then_some((b, seg_len.saturating_sub(b), base)),
    ]
}

/// Build one visual line's spans from its style zones, overlaying the reverse-video soft cursor when
/// `cursor` is `Some(col_within_seg)` (CMDHINT_01 restructure of the old cursor-overlay body).
///
/// The cursor cell is one whole GRAPHEME, not one char — pi takes `afterGraphemes[0].segment`
/// (`editor.ts:555-559`), so a ZWJ emoji inverts as a unit. The cluster is therefore measured
/// against the WHOLE remaining segment, never the zone, exactly as the code being replaced does.
/// That cannot straddle a zone edge; see the straddle proof in the task notes — the short form is
/// that the only non-trivial zone edge sits on a whitespace char, and no grapheme cluster spans
/// whitespace (GB3's `CR LF` is the sole exception and the buffer cannot contain `\r`). The `after`
/// slice is still clamped to the zone, which costs one `saturating_sub` and makes the property
/// enforced rather than merely argued.
pub(super) fn spans_for_segment(
    seg: &[char],
    zones: &[Option<(usize, usize, Style)>],
    cursor: Option<usize>,
    cursor_style: Style,
    base: Style,
) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    // `.flatten()` skips absent zones; the remaining ones are still left-to-right, which is what the
    // resulting span order depends on.
    for &(start, len, style) in zones.iter().flatten() {
        let end = start.saturating_add(len);
        match cursor.filter(|c| *c >= start && *c < end) {
            Some(c) => {
                let before: String =
                    seg.iter().skip(start).take(c.saturating_sub(start)).collect();
                if !before.is_empty() {
                    spans.push(Span::styled(before, style));
                }
                let tail: String = seg.iter().skip(c).collect();
                match tail.graphemes(true).next() {
                    Some(g) => {
                        let after_at = c.saturating_add(g.chars().count());
                        spans.push(Span::styled(g.to_string(), cursor_style));
                        let after: String = seg
                            .iter()
                            .skip(after_at)
                            .take(end.saturating_sub(after_at))
                            .collect();
                        if !after.is_empty() {
                            spans.push(Span::styled(after, style));
                        }
                    }
                    None => spans.push(Span::styled(" ", cursor_style)),
                }
            }
            None => {
                let text: String = seg.iter().skip(start).take(len).collect();
                if !text.is_empty() {
                    spans.push(Span::styled(text, style));
                }
            }
        }
    }
    // End-of-line caret: the cursor sits one past the last char (pi `editor.ts:563`). This is also
    // the whole-line case for an empty visual line, whose only zone is zero-length — the loop above
    // takes the `None` arm there (`0 >= 0 && 0 < 0` is false) and emits nothing, so this push is the
    // caret's only producer for an empty buffer.
    if cursor == Some(seg.len()) {
        spans.push(Span::styled(" ", cursor_style));
    }
    if spans.is_empty() {
        // An empty visual line that is NOT the cursor's. Today's code pushes an empty `base` span
        // here — keep `base`, not `cursor_style`, or a blank soft-newline row grows a stray caret.
        spans.push(Span::styled(String::new(), base));
    }
    spans
}

/// The dim ghost span for `hint`, clipped to `available` columns (CMDHINT_01).
///
/// Structurally safe by two independent mechanisms, which is why this is an affordance rather than a
/// layout guard: (1) the render `Paragraph` has **no** `.wrap(…)`, so ratatui truncates a
/// too-long `Line` instead of reflowing it — the ghost can never add a row; (2) the editor's height
/// comes from the wrap map of REAL buffer content (`visual_line_count`, `:567-571` ←
/// `app/layout.rs:71-77`), which the ghost is not part of. Clipped to `available - 1` chars plus `…`;
/// a single column is `…`.
pub(super) fn ghost_span(hint: &str, available: usize, style: Style) -> Option<Span<'static>> {
    if available == 0 {
        return None;
    }
    let n = hint.chars().count();
    let text = if n <= available {
        hint.to_string()
    } else if available == 1 {
        "…".to_string()
    } else {
        let mut s: String = hint.chars().take(available.saturating_sub(1)).collect();
        s.push('…');
        s
    };
    Some(Span::styled(text, style))
}

/// One scroll-indicator rule, a 1:1 port of `createScrollBorder` (`editor.ts:259-268`):
///
/// ```text
/// const indicator = `─── ${direction} ${hiddenLineCount} more `;
/// const remaining = availableWidth - visibleWidth(indicator);
/// if (remaining >= 0) return indicator + "─".repeat(remaining);
/// const ellipsis = "...".slice(0, availableWidth);
/// return sliceByColumn(indicator, 0, availableWidth - visibleWidth(ellipsis), true) + ellipsis;
/// ```
///
/// `direction` is `'↑'` (rows scrolled off the top) or `'↓'` (rows still below).
///
/// **The trailing `true` is `strict`, not a pad flag.** `sliceByColumn(line, startCol, length,
/// strict = false)` (`utils.ts:1195-1197`) forwards to `sliceWithWidth`, whose `strict` drops a
/// grapheme that would straddle the end column (`:1224`, `const fits = !strict || currentCol + w <=
/// endCol`); it returns `{ text, width }` and pads NOTHING. The `pad` parameter that does exist
/// upstream belongs to `truncateToWidth`/`finalizeTruncatedResult`, a different function. So
/// `createScrollBorder`'s fallback is not padded upstream and is not padded here — the loop below is
/// that strict slice, and it is equivalent statement for statement: upstream skips a non-fitting
/// grapheme and then breaks on `currentCol >= endCol`, which for a strictly-increasing column count
/// is the same set of graphemes this `break` keeps.
///
/// The result is nevertheless always exactly `width` display columns, and that is a property of the
/// indicator's ALPHABET rather than of the slice: `─`, the space, `↑`/`↓` (East-Asian Ambiguous,
/// hence narrow) and the decimal digits are every one of them a single column, so `strict` never has
/// a wide grapheme to reject. `the_scroll_rule_is_exactly_as_wide_as_it_is_asked_for` pins
/// it across the whole width range, because cyrup — unlike pi, which composes each row from scratch
/// — paints this string OVER the `Block`'s already-drawn rule, and a short string would leak the
/// `─`s underneath.
pub(super) fn scroll_border(direction: char, hidden: usize, width: u16) -> String {
    let avail = usize::from(width);
    let indicator = format!("─── {direction} {hidden} more ");
    let indicator_w = display_width(&indicator);
    if avail >= indicator_w {
        let mut out = indicator;
        out.push_str(&"─".repeat(avail - indicator_w));
        return out;
    }
    // Too narrow for the whole indicator: keep as many leading columns as fit, then `...` (itself
    // truncated to the available width on a truly tiny terminal).
    let ellipsis: String = "...".chars().take(avail).collect();
    let budget = avail.saturating_sub(display_width(&ellipsis));
    let mut out = String::new();
    let mut used = 0usize;
    for g in indicator.graphemes(true) {
        let w = display_width(g);
        if used + w > budget {
            break;
        }
        out.push_str(g);
        used += w;
    }
    out.push_str(&ellipsis);
    out
}

impl Component for InputEditor {
    /// Render the editor with **top + bottom rules only** (no side bars, no title) — Pi
    /// `editor.ts:476,517,575` (spec/tui/03 §3.1). The rule color flips to bash-green while the buffer
    /// starts with `!` (spec/tui/03 §7.1); otherwise it uses the border role, accented when focused.
    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        // Record the layout width so vertical (visual-line) motion wraps the same way it is drawn.
        // The editor has no side borders; one column is reserved for the end-of-line cursor cell
        // (`editor.ts:471` `layout_width = content_width - 1`) — unless `editorPaddingX` gave the
        // caret padding to overflow into (`editor.ts:489`).
        let pad = self.effective_padding(area.width);
        self.view_width = self.layout_width(area.width) as usize;
        // The rule color is the primary always-visible mode signal (spec/tui/03 §3.3): bash-green
        // while the buffer starts with `!`, else the escalating thinking-level color. The previous
        // hardwired bright-blue accent-on-focus was wrong (audit #3).
        let rule_style = if self.is_bash_mode() {
            theme.bash_mode_style()
        } else if self.thinking_level_owns_border {
            theme.thinking_border_style(&self.thinking_level)
        } else {
            // T9: an editor nobody reassigned keeps `getEditorTheme().borderColor` = `borderMuted`
            // (Pi `theme.ts:1301-1304` → `tui/src/components/editor.ts:348`).
            theme.border_muted_style()
        };
        // `editorPaddingX` insets the TEXT only: ratatui's `Block` draws its top/bottom rules across
        // the full `area` and applies `Padding` to the inner area the `Paragraph` fills, which is
        // exactly Pi's split (`editor.ts:522` pads the text rows; `:530` repeats the rule glyph
        // `width` times).
        let block = Block::default()
            .borders(Borders::TOP | Borders::BOTTOM)
            .border_set(border::PLAIN)
            .border_style(rule_style)
            .padding(Padding::horizontal(pad));
        // A reverse-video soft cursor cell makes the caret visible every idle frame (Pi
        // `editor.ts:545-564`). Without it the body row paints blank, because the hardware cursor
        // (`set_cursor_position`) is invisible in a headless buffer.
        //
        // **E1 — there is no prompt glyph.** cyrup used to open row 0 with an accent `› `. Pi's
        // `Editor.render` (`editor.ts:482-601`) emits only `${leftPadding}${displayText}${padding}
        // ${lineRightPadding}` (`:578`); nothing anywhere in the chat editor's construction adds a
        // leading glyph — the chat editor is a bare `new CustomEditor(this.ui, getEditorTheme(),
        // this.keybindings, {…})` (`interactive-mode.ts:563-566`) and `CustomEditor`
        // (`components/custom-editor.ts`, 90 lines) overrides `handleInput` ONLY, with no `render`.
        // The `›` upstream *does* draw is the SELECTED-ROW cursor of the list selectors
        // (`session-selector.ts:476`, `tree-selector.ts:689`, `user-message-selector.ts:57`), a
        // different component. Removing it also fixes **E2**: row 0 was `PROMPT_W + view_width`
        // columns wide inside a `view_width`-wide area (last character clipped, end-of-line caret off
        // the right edge) while rows 1..n started two columns to its left — a permanent ragged left
        // edge. Every row now starts flush at `leftPadding`, exactly as `:578` does.
        let base = theme.base_style();
        let cursor_style = base.add_modifier(Modifier::REVERSED);
        // Expand each LOGICAL line into its wrapped VISUAL lines at `view_width` (`editor.ts:1690`
        // `build_visual_line_map`, the same primitive vertical motion uses) and emit one ratatui
        // `Line` per visual line — so text past the width flows onto the next row instead of clipping
        // (the `Paragraph` has no `.wrap`, so it renders exactly the rows we build). The soft cursor
        // rides its VISUAL row/col, not the logical column.
        let map = self.visual_line_map();
        // **E13 — the caret survives focus loss.** Pi gates `focused` on the zero-width hardware
        // `CURSOR_MARKER` alone (`editor.ts:537,550`); the reverse-video cell is emitted purely from
        // `layoutLine.hasCursor`, which `layoutText` sets from the cursor position and never consults
        // `focused` (`editor.ts:905-960`). cyrup used to set `cursor_vl = usize::MAX` when unfocused,
        // so clicking away from the terminal (`FocusLost`) erased the caret entirely. The
        // focus-gated half lives on in [`Self::cursor_in`], which is cyrup's `CURSOR_MARKER`.
        let cursor_vl = self.current_visual_line(&map);
        // **E4 — the visible window scrolls; it does not clip.** Pi slices `layoutLines` to
        // `maxVisibleLines` after moving `scrollOffset` to keep the caret inside
        // (`editor.ts:499-519`).
        //
        // **E17 — the cap is the component's own.** Upstream reads `this.tui.terminal.rows` inside a
        // `render(width)` that takes no height (`:499-501`); the budget is intrinsic and the
        // container is never consulted. cyrup took `area.height - 2` alone — correct only for as
        // long as the one caller happened to size the slot from the same formula, and silently
        // uncapped for every other caller. It is now `min(rect, intrinsic)`: the intrinsic budget is
        // pi's, and the rect stays in the `min` so a slot CLIPPED shorter than the editor asked for
        // still degrades correctly rather than overdrawing its neighbours.
        let max_visible = usize::from(area.height.saturating_sub(2))
            .min(usize::from(self.max_visible_lines()))
            .max(1);
        if cursor_vl < self.scroll_offset {
            self.scroll_offset = cursor_vl;
        } else if cursor_vl >= self.scroll_offset.saturating_add(max_visible) {
            self.scroll_offset = cursor_vl.saturating_add(1).saturating_sub(max_visible);
        }
        self.scroll_offset = self.scroll_offset.min(map.len().saturating_sub(max_visible));
        let mut lines: Vec<Line> = Vec::with_capacity(max_visible);
        let highlight = self.command_highlight();          // computed ONCE per frame
        let accent = theme.accent_style();
        let dim = theme.dim_style();
        let last_vi = map.len().saturating_sub(1);
        // The Block's inner width — the true drawable span for the ghost. NOT `self.view_width`:
        // `layout_width` subtracts one column for the caret when `paddingX == 0`, so `view_width`
        // under-counts the inner area by one in the default configuration. The formula is exactly
        // `area.width - 2 * pad` because the Block carries `Borders::TOP | BOTTOM` only — no side
        // border steals a column — and `Padding::horizontal(pad)` with `pad = effective_padding(...)`.
        let inner_w = usize::from(area.width.saturating_sub(pad.saturating_mul(2))).max(1);
        for (vi, vl) in map.iter().enumerate().skip(self.scroll_offset).take(max_visible) {
            // The chars this visual line slices out of its logical line.
            let seg: Vec<char> = self
                .lines
                .get(vl.logical)
                .map(|l| l.iter().skip(vl.start).take(vl.len).copied().collect())
                .unwrap_or_default();
            let zones = style_zones(vl, seg.len(), highlight.as_ref().map(|h| &h.token), base, accent);
            let cursor = (vi == cursor_vl).then(|| self.col.saturating_sub(vl.start).min(seg.len()));
            let mut spans = spans_for_segment(&seg, &zones, cursor, cursor_style, base);
            // The ghost trails the buffer's LAST visual line, after the real content and AFTER the
            // caret cell. It is not buffer content, so it never joins the cursor split and the cursor
            // can never sit inside it. Deliberately after the caret, not under it: cyrup's caret is a
            // reverse-video BLOCK, and a dim hint char inverted beneath it would read as
            // already-typed text — the opposite of what a placeholder must say.
            if vi == last_vi
                && let Some(hint) = highlight.as_ref().and_then(|h| h.ghost.as_deref())
            {
                // Charge COLUMNS, not chars — and take them from the spans just built, so the caret
                // cell (a `" "` span, or the inverted grapheme) is counted by construction rather
                // than re-derived, and no intermediate `String` is allocated.
                let used: usize = spans.iter().map(Span::width).sum();
                if let Some(span) = ghost_span(hint, inner_w.saturating_sub(used), dim) {
                    spans.push(span);
                }
            }
            lines.push(Line::from(spans));
        }
        let shown = lines.len();
        let para = Paragraph::new(lines).block(block).style(base);
        frame.render_widget(para, area);
        // E4's other half: the rules ANNOUNCE the hidden rows. `createScrollBorder`
        // (`editor.ts:259-268`) replaces the plain `─`-repeat with `─── ↑ N more ───…` at the top
        // when `scrollOffset > 0` (`:526-528`) and `─── ↓ N more ───…` at the bottom when content
        // remains below (`:582-585`). The `Block` above already painted a plain rule on both edges;
        // these overwrite it in place, which is byte-identical to pi choosing one string or the other
        // (both are exactly `width` columns).
        if self.scroll_offset > 0 && area.height >= 1 {
            let text = scroll_border('↑', self.scroll_offset, area.width);
            let row = Rect { x: area.x, y: area.y, width: area.width, height: 1 };
            frame.render_widget(Paragraph::new(Line::from(Span::styled(text, rule_style))), row);
        }
        let below = map.len().saturating_sub(self.scroll_offset.saturating_add(shown));
        if below > 0 && area.height >= 2 {
            let text = scroll_border('↓', below, area.width);
            let row = Rect {
                x: area.x,
                y: area.y.saturating_add(area.height.saturating_sub(1)),
                width: area.width,
                height: 1,
            };
            frame.render_widget(Paragraph::new(Line::from(Span::styled(text, rule_style))), row);
        }
        // Pi hides the terminal's real cursor unless `showHardwareCursor` is on (`tui.ts:1659-1663`
        // `if (this.showHardwareCursor) showCursor() else hideCursor()`); ratatui's `Terminal::draw`
        // hides it for us whenever no position was set, so the gate is the call itself.
        if self.show_hardware_cursor
            && let Some((x, y)) = self.cursor_in(area)
        {
            frame.set_cursor_position((x, y));
        }
    }
}
