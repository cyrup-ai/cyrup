//! Markdown + syntax rendering of assistant / message bodies (spec/tui/06 §2-§3; R-10-012).
//!
//! A 1:1-in-spirit port of `pi-tui/src/components/markdown.ts`: a `pulldown-cmark` event walk that
//! produces styled `ratatui` [`Line`]s at a fixed content width, plus `syntect`-driven syntax
//! highlighting of fenced code blocks (spec/tui/06 §3, `utils/syntax-highlight.ts`).
//!
//! ## Faithful behaviors
//! - **Headings** — H1/H2 drop the `#` prefix and bold (H1 also underlines); H3–H6 keep a literal
//!   `### ` prefix then bold, with a trailing blank line (`markdown.ts:336-362`).
//! - **Lists** — `- ` unordered (in `mdListBullet`), `N. ` ordered renumbered from `start`, and
//!   `[ ] `/`[x] ` task markers; nesting indents two columns per level (`markdown.ts:591-654`).
//! - **Blockquote** — each line prefixed `│ ` in `mdQuoteBorder`, body italic in `mdQuote`
//!   (`markdown.ts:414-461`).
//! - **Horizontal rule** — `─` × `min(width, 80)` in `mdHr` (`markdown.ts:463-468`).
//! - **Fenced code** — literal ```` ``` ````+info fence lines in `mdCodeBlockBorder`, a 2-space indent
//!   per code line, syntect highlighting when the language is explicitly known (auto-detect **off**,
//!   spec/tui/06 §3.1) else a flat `mdCodeBlock` body (`markdown.ts:378-398`).
//! - **Inline** — bold/italic/strikethrough, inline code in `mdCode` (no backticks), links underlined
//!   in `mdLink` with a trailing ` (url)` in `mdLinkUrl` when the text differs (`markdown.ts:492-589`).
//! - **Streaming partial-fence trim** ([`trim_partial_closing_fence`]) keeps a streaming code block
//!   from flickering open/closed as the closing fence arrives char-by-char (`markdown.ts:25-48`).

use std::sync::OnceLock;

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::parsing::{ParseState, ScopeStack, SyntaxSet};

use crate::theme::UiTheme;

/// Render markdown `text` into styled lines at content `width` (spec/tui/06 §2). Total / never panics:
/// any structure pulldown-cmark cannot parse degrades to plain text spans.
pub fn render(text: &str, width: usize, theme: &UiTheme) -> Vec<Line<'static>> {
    render_with_text_color(text, width, theme, None)
}

/// [`render`] with an explicit default **prose** colour — Pi's `Markdown(text, …, { color })`
/// option (`tui/src/components/markdown.ts:182-183`), applied through `applyDefaultStyle` /
/// `getDefaultStylePrefix` (`:377-404`, `:406-438`) to the inline text runs only. Headings, code,
/// quotes, rules and links keep their own `MarkdownTheme` colours upstream and here, which is why
/// this replaces the plain-paragraph arm of [`MdRenderer::inline_style`] and nothing else.
///
/// `None` keeps the ordinary `text`-role prose colour.
pub fn render_with_text_color(
    text: &str,
    width: usize,
    theme: &UiTheme,
    color: Option<ratatui::style::Color>,
) -> Vec<Line<'static>> {
    render_with_default_style(text, width, theme, color, false)
}

/// [`render_with_text_color`] plus the `italic` leg of Pi's `defaultTextStyle`
/// (`applyDefaultStyle`, `tui/src/components/markdown.ts:377-404`: `color` then `bold`, `italic`,
/// `strikethrough`, `underline`, in that order). Only `color` + `italic` have a caller in
/// coding-agent — the reasoning block, `new Markdown(thinkingBlocks.join("\n\n"), outputPad, 0,
/// markdownTheme, { color: (t) => theme.fg("thinkingText", t), italic: true }, …)`
/// (`assistant-message.ts:146-164`).
///
/// Both legs land in the same place the colour does — the plain-prose arm of
/// [`MdRenderer::inline_style`] — because upstream reaches them only through
/// `getDefaultInlineStyleContext()` (`markdown.ts:447-452`), which `renderToken` passes to the
/// `paragraph`/`text` arms. `heading` builds its own context (`:470-480`) and `code` never consults
/// one (`:520-539`), so a `## Plan` inside a thinking block keeps `mdHeading` and a fence keeps its
/// syntax colours, exactly as upstream.
pub fn render_with_default_style(
    text: &str,
    width: usize,
    theme: &UiTheme,
    color: Option<ratatui::style::Color>,
    italic: bool,
) -> Vec<Line<'static>> {
    // Tabs → 3 spaces before parse (`markdown.ts:171`).
    let prepared = text.replace('\t', "   ");
    let mut r = MdRenderer::new(width, theme);
    r.default_text = color;
    r.default_italic = italic;
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    for ev in Parser::new_ext(&prepared, opts) {
        r.event(ev);
    }
    r.finish()
}

/// Trim a *partial* closing code fence from a streaming buffer so the live markdown block does not
/// flip between open/closed while the fence (`` ` `` → `` `` `` → ```` ``` ````) streams in
/// (`markdown.ts:25-48`, pi#5825). Only the **last** line is inspected; apply to the live buffer only.
pub fn trim_partial_closing_fence(text: &str) -> String {
    // Count fence markers (lines that are exactly N backticks/tildes after trimming). An *odd* count
    // means a code block is currently open; a trailing line that is a *short* run of the same fence
    // char is a partial closing fence and is stripped to keep the block stable.
    let mut fence_char: Option<char> = None;
    let mut open = false;
    let mut open_len = 0usize;
    for line in text.lines() {
        let t = line.trim();
        if !open {
            // Opening fence: a leading run of ≥3 fence chars (an info string may follow, e.g. ```rust).
            if let Some((c, n)) = leading_fence(t) {
                open = true;
                fence_char = Some(c);
                open_len = n;
            }
        } else if let Some((c, n, pure)) = leading_fence(t).map(|(c, n)| (c, n, is_pure_fence(t))) {
            // Closing fence must be a *pure* run of the same char, at least as long as the opener.
            if pure && Some(c) == fence_char && n >= open_len {
                open = false;
                fence_char = None;
            }
        }
    }
    if !open {
        return text.to_string();
    }
    // A code block is open. If the final line is a *partial* fence (same char, shorter than the
    // opener), drop it so the renderer keeps showing the open block unchanged.
    let last_start = text.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let last = text.get(last_start..).unwrap_or("").trim();
    if !last.is_empty()
        && let Some(fc) = fence_char
        && last.chars().all(|c| c == fc)
        && last.chars().count() < open_len
    {
        return text.get(..last_start.saturating_sub(1)).unwrap_or("").to_string();
    }
    text.to_string()
}

/// If `line` begins with a run of ≥3 fence chars (all `` ` `` or all `~`), return its char + run
/// length. An info string may follow (opening fence ```` ```rust ````).
fn leading_fence(line: &str) -> Option<(char, usize)> {
    let first = line.chars().next()?;
    if first != '`' && first != '~' {
        return None;
    }
    let n = line.chars().take_while(|c| *c == first).count();
    if n >= 3 { Some((first, n)) } else { None }
}

/// Whether `line` is *only* a fence run (a valid closing fence — no trailing info string).
fn is_pure_fence(line: &str) -> bool {
    match line.chars().next() {
        Some(c @ ('`' | '~')) => line.chars().all(|x| x == c) && line.chars().count() >= 3,
        _ => false,
    }
}

/// The renderer's running state during the event walk.
struct MdRenderer<'t> {
    width: usize,
    theme: &'t UiTheme,
    out: Vec<Line<'static>>,
    cur: Vec<Span<'static>>,
    /// Active inline emphasis depth counters.
    bold: u32,
    italic: u32,
    strike: u32,
    /// Heading level currently open (style override + trailing blank line).
    heading: Option<HeadingLevel>,
    /// Blockquote nesting depth (each level adds a `│ ` prefix + italic body).
    quote: u32,
    /// List context stack: `Some(next_number)` ordered, `None` unordered.
    lists: Vec<Option<u64>>,
    /// Pending list-item marker (emitted lazily before the item's first inline text).
    pending_marker: Option<(String, Style)>,
    /// Active link href (the trailing ` (url)` is emitted on link end).
    link: Option<String>,
    /// Buffered link text (to compare against the href for the trailing-url rule).
    link_text: String,
    /// Fenced-code capture: `Some(lang)` while inside a code block.
    code_lang: Option<String>,
    code_buf: String,
    /// Simple table capture (header row + body rows of plain-text cells).
    table: Option<TableCapture>,
    /// Pi's `Markdown` `{ color }` option: the default foreground for plain prose runs, replacing
    /// the `text` role. `None` ⇒ the ordinary assistant/body colour.
    default_text: Option<ratatui::style::Color>,
    /// Pi's `Markdown` `{ italic }` option — the second leg of `applyDefaultStyle`
    /// (`markdown.ts:393-395`). Applied alongside [`Self::default_text`] on plain prose only.
    default_italic: bool,
}

#[derive(Default)]
struct TableCapture {
    in_head: bool,
    cur_cell: String,
    cur_row: Vec<String>,
    header: Vec<String>,
    rows: Vec<Vec<String>>,
}

impl<'t> MdRenderer<'t> {
    fn new(width: usize, theme: &'t UiTheme) -> Self {
        MdRenderer {
            width,
            theme,
            out: Vec::new(),
            cur: Vec::new(),
            bold: 0,
            italic: 0,
            strike: 0,
            heading: None,
            quote: 0,
            lists: Vec::new(),
            pending_marker: None,
            link: None,
            link_text: String::new(),
            code_lang: None,
            code_buf: String::new(),
            table: None,
            default_text: None,
            default_italic: false,
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
            if self.default_italic { base.add_modifier(Modifier::ITALIC) } else { base }
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
    fn push_text(&mut self, text: &str, style: Style) {
        if let Some((marker, mstyle)) = self.pending_marker.take() {
            self.start_line_prefix();
            self.cur.push(Span::styled(marker, mstyle));
        } else if self.cur.is_empty() {
            self.start_line_prefix();
        }
        if self.link.is_some() {
            self.link_text.push_str(text);
        }
        self.cur.push(Span::styled(text.to_string(), style));
    }

    /// Emit the leading quote/indent prefix at the start of a fresh line.
    fn start_line_prefix(&mut self) {
        for _ in 0..self.quote {
            self.cur.push(Span::styled("│ ".to_string(), self.theme.md_quote_border_style()));
        }
        let depth = self.lists.len().saturating_sub(1);
        if depth > 0 {
            self.cur.push(Span::raw("  ".repeat(depth)));
        }
    }

    /// Flush the current spans as one output line (no-op when empty).
    fn flush_line(&mut self) {
        if !self.cur.is_empty() {
            self.out.push(Line::from(std::mem::take(&mut self.cur)));
        }
    }

    /// Push a blank separator line unless the output already ends with one.
    fn blank(&mut self) {
        let trailing_blank = self
            .out
            .last()
            .map(|l| l.spans.iter().all(|s| s.content.trim().is_empty()))
            .unwrap_or(true);
        if !trailing_blank {
            self.out.push(Line::default());
        }
    }

    fn event(&mut self, ev: Event<'_>) {
        match ev {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => {
                if let Some(table) = self.table.as_mut() {
                    table.cur_cell.push_str(&t);
                } else if self.code_lang.is_some() {
                    self.code_buf.push_str(&t);
                } else {
                    let style = self.inline_style();
                    self.push_text(&t, style);
                }
            }
            Event::Code(c) => {
                if let Some(table) = self.table.as_mut() {
                    table.cur_cell.push_str(&c);
                } else {
                    let style = self.theme.md_code_style();
                    self.push_text(&c, style);
                }
            }
            Event::SoftBreak => {
                if self.table.is_none() {
                    let style = self.inline_style();
                    self.push_text(" ", style);
                }
            }
            Event::HardBreak => self.flush_line(),
            Event::Rule => {
                self.flush_line();
                let n = self.width.clamp(1, 80);
                self.out.push(Line::styled("─".repeat(n), self.theme.md_hr_style()));
            }
            Event::TaskListMarker(checked) => {
                let mark = if checked { "[x] " } else { "[ ] " };
                // Replace the bullet marker that the list item already queued.
                self.pending_marker = Some((mark.to_string(), self.theme.md_list_bullet_style()));
            }
            Event::Html(h) | Event::InlineHtml(h) => {
                let style = self.inline_style();
                self.push_text(h.trim_end_matches('\n'), style);
            }
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
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
            Tag::List(start) => {
                self.flush_line();
                self.lists.push(start);
            }
            Tag::Item => {
                let depth = self.lists.len().saturating_sub(1);
                let marker = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        let s = format!("{n}. ");
                        *n = n.saturating_add(1);
                        s
                    }
                    _ => "- ".to_string(),
                };
                let _ = depth;
                self.pending_marker = Some((marker, self.theme.md_list_bullet_style()));
            }
            Tag::BlockQuote(_) => {
                self.flush_line();
                self.quote = self.quote.saturating_add(1);
            }
            Tag::CodeBlock(kind) => {
                self.flush_line();
                self.code_lang = Some(match kind {
                    pulldown_cmark::CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().unwrap_or("").to_string()
                    }
                    pulldown_cmark::CodeBlockKind::Indented => String::new(),
                });
                self.code_buf.clear();
            }
            Tag::Emphasis => self.italic = self.italic.saturating_add(1),
            Tag::Strong => self.bold = self.bold.saturating_add(1),
            Tag::Strikethrough => self.strike = self.strike.saturating_add(1),
            Tag::Link { dest_url, .. } => {
                self.link = Some(dest_url.to_string());
                self.link_text.clear();
            }
            Tag::Table(_) => {
                self.flush_line();
                self.table = Some(TableCapture::default());
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
            TagEnd::List(_) => {
                self.lists.pop();
                if self.lists.is_empty() {
                    self.blank();
                }
            }
            TagEnd::Item => self.flush_line(),
            TagEnd::BlockQuote(_) => {
                self.flush_line();
                self.quote = self.quote.saturating_sub(1);
                if self.quote == 0 {
                    self.blank();
                }
            }
            TagEnd::CodeBlock => {
                let lang = self.code_lang.take().unwrap_or_default();
                let code = std::mem::take(&mut self.code_buf);
                self.emit_code_block(&lang, &code);
            }
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link => {
                if let Some(href) = self.link.take() {
                    let text = std::mem::take(&mut self.link_text);
                    let stripped = href.strip_prefix("mailto:").unwrap_or(&href);
                    if !text.is_empty() && text != stripped {
                        let style = self.theme.md_link_url_style();
                        self.push_text(&format!(" ({href})"), style);
                    }
                }
            }
            TagEnd::Table => self.emit_table(),
            TagEnd::TableCell => {
                if let Some(t) = self.table.as_mut() {
                    let cell = std::mem::take(&mut t.cur_cell);
                    t.cur_row.push(cell.trim().to_string());
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

    /// Emit a fenced code block: top fence line, highlighted (or flat) body, bottom fence line.
    fn emit_code_block(&mut self, lang: &str, code: &str) {
        let border = self.theme.md_code_block_border_style();
        self.out.push(Line::styled(format!("```{lang}"), border));
        for line in highlight_lines(code, lang, self.theme) {
            self.out.push(line);
        }
        self.out.push(Line::styled("```".to_string(), border));
        self.blank();
    }

    /// Emit the captured table as a full box-drawing grid (`┌┬┐ ├┼┤ └┴┘ │ ─`), a 1:1 port of
    /// `markdown.ts:685-856` `renderTable`: per-column width fitting to the content width, a bold
    /// header band, a `├─┼─┤` separator between **every** row, and width-aware cell wrapping. Closes
    /// gap 12 (tables were previously ` │ `-joined, no grid). Border rows render in `mdHr`, the header
    /// bold, body cells in the base style.
    fn emit_table(&mut self) {
        let Some(t) = self.table.take() else { return };
        let num_cols = t.header.len();
        if num_cols == 0 {
            return;
        }
        // Border overhead = "│ " + (n-1)*" │ " + " │" = 3n + 1 (`markdown.ts:700`).
        let overhead = 3usize.saturating_mul(num_cols).saturating_add(1);
        let avail = self.width.max(overhead.saturating_add(num_cols));
        let avail_cells = avail.saturating_sub(overhead);

        // Natural width per column = widest visible cell (header + body), clamped to ≥1. Index-free.
        let natural: Vec<usize> = (0..num_cols)
            .map(|c| {
                let head_w = t.header.get(c).map(|h| display_width(h)).unwrap_or(0);
                let body_w = t
                    .rows
                    .iter()
                    .filter_map(|r| r.get(c))
                    .map(|cell| display_width(cell))
                    .max()
                    .unwrap_or(0);
                head_w.max(body_w).max(1)
            })
            .collect();
        // Fit: if the natural total overflows, shrink each column proportionally toward a floor of 1
        // (`markdown.ts:761-800`); otherwise keep natural widths.
        let total_natural: usize = natural.iter().sum();
        let widths: Vec<usize> = if total_natural <= avail_cells {
            natural.clone()
        } else {
            let extra = avail_cells.saturating_sub(num_cols);
            let grow_potential: usize = natural.iter().map(|w| w.saturating_sub(1)).sum();
            let mut w: Vec<usize> = natural
                .iter()
                .map(|n| {
                    let delta = n.saturating_sub(1);
                    let grow = delta.saturating_mul(extra).checked_div(grow_potential).unwrap_or(0);
                    1 + grow
                })
                .collect();
            // Distribute rounding leftovers left-to-right, never past the natural width.
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

        let hr = self.theme.md_hr_style();
        let heading = self.theme.md_heading_style();
        let base = self.theme.assistant_style();
        let border = |left: &str, mid: &str, right: &str, ws: &[usize]| -> Line<'static> {
            let cells: Vec<String> = ws.iter().map(|w| "─".repeat(*w)).collect();
            Line::styled(format!("{left}{}{right}", cells.join(mid)), hr)
        };

        // Top border ┌─...─┬─...─┐.
        self.out.push(border("┌─", "─┬─", "─┐", &widths));
        // Header band (bold), wrapped.
        self.push_table_row(&t.header, &widths, heading, '│');
        // Separator ├─...─┼─...─┤.
        let sep = || border("├─", "─┼─", "─┤", &widths);
        self.out.push(sep());
        for (ri, row) in t.rows.iter().enumerate() {
            self.push_table_row(row, &widths, base, '│');
            if ri + 1 < t.rows.len() {
                self.out.push(sep());
            }
        }
        // Bottom border └─...─┴─...─┘.
        self.out.push(border("└─", "─┴─", "─┘", &widths));
        self.blank();
    }

    /// Render one table row of `cells` into `│ … │` lines, wrapping each cell to its column width and
    /// padding short cells with spaces (`markdown.ts:806-842`). The `│` separators render in `mdHr`,
    /// the cell text in `cell_style`.
    fn push_table_row(&mut self, cells: &[String], widths: &[usize], cell_style: Style, bar: char) {
        let hr = self.theme.md_hr_style();
        let wrapped: Vec<Vec<String>> = widths
            .iter()
            .enumerate()
            .map(|(i, w)| wrap_cell(cells.get(i).map(String::as_str).unwrap_or(""), *w))
            .collect();
        let height = wrapped.iter().map(Vec::len).max().unwrap_or(1).max(1);
        for li in 0..height {
            let mut spans: Vec<Span<'static>> = Vec::with_capacity(widths.len() * 2 + 1);
            spans.push(Span::styled(format!("{bar} "), hr));
            for (ci, w) in widths.iter().enumerate() {
                if ci > 0 {
                    spans.push(Span::styled(format!(" {bar} "), hr));
                }
                let text = wrapped.get(ci).and_then(|c| c.get(li)).cloned().unwrap_or_default();
                let pad = w.saturating_sub(display_width(&text));
                spans.push(Span::styled(format!("{text}{}", " ".repeat(pad)), cell_style));
            }
            spans.push(Span::styled(format!(" {bar}"), hr));
            self.out.push(Line::from(spans));
        }
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        self.flush_line();
        // Drop a single trailing blank line for tight scrollback packing.
        if self.out.last().map(|l| l.spans.iter().all(|s| s.content.trim().is_empty())).unwrap_or(false)
        {
            self.out.pop();
        }
        self.out
    }
}

/// Visible (terminal-column) width of `s`, unicode-width-correct via ratatui's `Span::width`
/// (matches Pi's `visibleWidth`). Never `str::len`.
fn display_width(s: &str) -> usize {
    Span::raw(s).width()
}

/// Greedy word-wrap `text` to a column of `width` cells (`markdown.ts:672` `wrapCellText`): split on
/// spaces, pack words onto a line, and hard-break any single word wider than the column. Always
/// returns at least one (possibly empty) line so a cell occupies a row.
fn wrap_cell(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out: Vec<String> = Vec::new();
    let mut line = String::new();
    let mut line_w = 0usize;
    for word in text.split_whitespace() {
        let ww = display_width(word);
        if ww > width {
            // Flush the current line, then hard-break the long word into width-sized chunks.
            if !line.is_empty() {
                out.push(std::mem::take(&mut line));
                line_w = 0;
            }
            let mut chunk = String::new();
            let mut chunk_w = 0usize;
            for ch in word.chars() {
                let cw = display_width(ch.encode_utf8(&mut [0u8; 4]));
                if chunk_w + cw > width {
                    out.push(std::mem::take(&mut chunk));
                    chunk_w = 0;
                }
                chunk.push(ch);
                chunk_w += cw;
            }
            if !chunk.is_empty() {
                line = chunk;
                line_w = chunk_w;
            }
            continue;
        }
        let sep = usize::from(!line.is_empty());
        if line_w + sep + ww > width {
            out.push(std::mem::take(&mut line));
            line.push_str(word);
            line_w = ww;
        } else {
            if sep == 1 {
                line.push(' ');
            }
            line.push_str(word);
            line_w += sep + ww;
        }
    }
    out.push(line);
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn heading_depth(level: HeadingLevel) -> usize {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// The shared default syntect syntax set (newline-terminated grammars), built once.
fn syntax_set() -> &'static SyntaxSet {
    static SS: OnceLock<SyntaxSet> = OnceLock::new();
    SS.get_or_init(SyntaxSet::load_defaults_newlines)
}

/// Highlight `code` as `lang` into 2-space-indented styled lines (spec/tui/06 §3). When the language
/// is unknown to syntect, every line renders flat in `mdCodeBlock` (auto-detect-off parity, §3.1); on
/// any syntect error the whole block falls back to flat (mirrors `theme.ts:1142-1146` try/catch).
fn highlight_lines(code: &str, lang: &str, theme: &UiTheme) -> Vec<Line<'static>> {
    let flat = || -> Vec<Line<'static>> {
        code.split('\n')
            .map(|l| Line::styled(format!("  {l}"), theme.md_code_block_style()))
            .collect()
    };
    let token = lang.trim();
    if token.is_empty() {
        return flat();
    }
    let ss = syntax_set();
    let Some(syntax) = ss.find_syntax_by_token(token) else {
        return flat();
    };
    match highlight_inner(code, syntax, ss, theme) {
        Some(lines) if !lines.is_empty() => lines,
        _ => flat(),
    }
}

/// Stateful syntect highlight: parse each line, walk the scope stack, map the top matching scope to a
/// theme syntax role (spec/tui/06 §3.2). Returns `None` on any parser/scope error → caller falls back.
fn highlight_inner(
    code: &str,
    syntax: &syntect::parsing::SyntaxReference,
    ss: &SyntaxSet,
    theme: &UiTheme,
) -> Option<Vec<Line<'static>>> {
    let mut parse = ParseState::new(syntax);
    let mut out: Vec<Line<'static>> = Vec::new();
    for raw in code.split('\n') {
        let line_nl = format!("{raw}\n");
        let ops = parse.parse_line(&line_nl, ss).ok()?;
        let mut stack = ScopeStack::new();
        let mut spans: Vec<Span<'static>> = vec![Span::raw("  ")];
        let mut last = 0usize;
        for (idx, op) in ops {
            if idx > last
                && let Some(piece) = line_nl.get(last..idx)
            {
                push_code_span(&mut spans, piece, &stack, theme);
            }
            stack.apply(&op).ok()?;
            last = idx;
        }
        if let Some(piece) = line_nl.get(last..) {
            push_code_span(&mut spans, piece, &stack, theme);
        }
        out.push(Line::from(spans));
    }
    Some(out)
}

/// Push a highlighted span (newline-stripped) styled by the most specific matching scope.
///
/// T5 (TUI-FIDELITY §2): a scope the table does not classify gets **no style at all**, not
/// `mdCodeBlock`. Pi runs the block through cli-highlight and pushes the result verbatim —
/// `lines.push(`${indent}${hlLine}`)`, v0.84.1 `tui/src/components/markdown.ts:526` — and
/// cli-highlight only emits an escape for the 24 classes `buildCliHighlightTheme` defines
/// (`theme.ts:1119-1145`). Everything else (identifiers, whitespace, plain text) carries no escape
/// and renders at the terminal's default foreground. `mdCodeBlock` is a *whole-block* fallback in
/// Pi, reached only when the language is unknown or the highlighter throws (`theme.ts:1275`,
/// `:1284`); that path is [`highlight_lines`]'s `flat()`, not this one. Defaulting each unclassified
/// run to `mdCodeBlock` painted roughly half of every code block `#b5bd68` green.
fn push_code_span(spans: &mut Vec<Span<'static>>, piece: &str, stack: &ScopeStack, theme: &UiTheme) {
    let text = piece.trim_end_matches('\n');
    if text.is_empty() {
        return;
    }
    let style = scope_style(stack, theme).unwrap_or_default();
    spans.push(Span::styled(text.to_string(), style));
}

/// Map the scope stack to a theme syntax style.
///
/// Two passes, in this order:
/// 1. **Container scopes** (T6) — an enclosing `meta.annotation` / `meta.preprocessor` colours the
///    whole construct `muted`, because Pi's highlighter emits a `meta` class for a Rust attribute /
///    Python decorator / C preprocessor line and maps it to `muted` (v0.84.1 `theme.ts:1128`). This
///    has to beat the deepest-first walk: syntect nests `punctuation.definition.annotation.rust`
///    *inside* `meta.annotation.rust`, so a deepest-first match would recolour only the `#`.
///    A nested **string/comment literal escapes** the container and keeps its own colour, because
///    highlight.js's `meta` modes declare sub-modes that cli-highlight wraps in their own class —
///    see [`UiTheme::syntax_meta_nested_style`]. That is what keeps the `"wasm-host"` in
///    `#[cfg(feature = "wasm-host")]` and the `<stdio.h>` in `#include <stdio.h>` at
///    `syntaxString` while the annotation around them stays `muted`.
/// 2. **Deepest-first** — the innermost scope that the prefix table knows wins, so a `string` inside
///    a `meta.function` still comes out as a string.
fn scope_style(stack: &ScopeStack, theme: &UiTheme) -> Option<Style> {
    let container = stack
        .as_slice()
        .iter()
        .find_map(|scope| theme.syntax_meta_container_style(&scope.build_string()));
    if let Some(container) = container {
        for scope in stack.as_slice().iter().rev() {
            if let Some(style) = theme.syntax_meta_nested_style(&scope.build_string()) {
                return Some(style);
            }
        }
        return Some(container);
    }
    for scope in stack.as_slice().iter().rev() {
        let s = scope.build_string();
        if let Some(style) = theme.syntax_style_for_scope(&s) {
            return Some(style);
        }
    }
    None
}

