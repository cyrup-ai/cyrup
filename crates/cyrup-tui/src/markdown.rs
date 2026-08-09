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
//!   `- [ ] `/`- [x] ` task markers (the box is appended to the bullet, `markdown.ts:770-774`);
//!   nesting indents **four** columns per level (`markdown.ts:758`).
//! - **Blockquote** — each line prefixed `│ ` in `mdQuoteBorder`, body italic in `mdQuote`
//!   (`markdown.ts:414-461`).
//! - **Horizontal rule** — `─` × `min(width, 80)` in `mdHr` (`markdown.ts:463-468`).
//! - **Fenced code** — literal ```` ``` ````+info fence lines in `mdCodeBlockBorder`, a 2-space indent
//!   per code line, syntect highlighting when the language is explicitly known (auto-detect **off**,
//!   spec/tui/06 §3.1) else a flat `mdCodeBlock` body (`markdown.ts:378-398`).
//! - **Inline** — bold/italic/strikethrough (`~~` only — Pi's `StrictStrikethroughTokenizer`,
//!   `markdown.ts:7-24`), inline code in `mdCode` (no backticks), links underlined in `mdLink` with a
//!   trailing ` (url)` in `mdLinkUrl` when the text differs **and** the terminal cannot render OSC-8
//!   (`markdown.ts:689-708`).
//! - **Soft line breaks** — a `\n` inside a paragraph stays a row break, because marked leaves it in
//!   the text token and `wrapTextWithAnsi` splits on it (`markdown.ts:638-641`, `utils.ts:839`).
//! - **Streaming partial-fence trim** ([`trim_partial_closing_fence`]) keeps a streaming code block
//!   from flickering open/closed as the closing fence arrives char-by-char (`markdown.ts:25-48`).
//! - **Wrapping** — every logical row is wrapped to the width of the container it is in and then
//!   re-prefixed, upstream's order at all three of its wrapping sites (`markdown.ts:322`+`:340`,
//!   `:594-597`, `:788-791`). See [`MdRenderer::flush_line`].
//! - **Double wrap** — after the token walk, EVERY produced row goes through the wrap once more at
//!   the component's own `contentWidth` (`markdown.ts:316-326`). See [`MdRenderer::finish`].
//!
//! ## The prefix machinery, and the rule for adding to it
//! Upstream every block token returns a bare `string[]` from `renderToken` and its **caller**
//! decorates it: `quoteBorder("│ ") + wrappedLine` inside a blockquote (`markdown.ts:596`),
//! `linePrefix + wrappedLine` inside a list item (`:790`). cyrup's single event walk has no such
//! caller, so **every** row must leave through [`MdRenderer::flush_line`] —
//! [`MdRenderer::emit_prefixed`] for an already-built [`Line`], [`MdRenderer::blank`] for a
//! separator. A `self.out.push(…)` anywhere else silently drops the blockquote border and swallows
//! a queued list bullet; that was the shared root cause of three separate defects in `emit_table`,
//! `Event::Rule` and `emit_code_block`.

use std::sync::OnceLock;

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::parsing::{ParseState, ScopeStack, SyntaxSet};
use unicode_segmentation::UnicodeSegmentation;

use crate::theme::UiTheme;

/// Render markdown `text` into styled lines at content `width` (spec/tui/06 §2). Total / never panics:
/// any structure pulldown-cmark cannot parse degrades to plain text spans.
///
/// **`width` is the CONTENT width, and the wrap happens in here.** Upstream `Markdown.render(width)`
/// takes the COMPONENT width and derives `contentWidth = Math.max(1, width - this.paddingX * 2)`
/// itself (`markdown.ts:284`), because the component owns its `paddingX`. In cyrup the CALLER owns
/// it, and — this is the part an earlier revision of this comment got wrong — the six call sites do
/// **not** all reduce by `output_pad * 2`. Each passes the content width of whatever container it
/// sits in, which is precisely what the seam is for:
///
/// | call site (verified at HEAD) | `width` passed | upstream shape |
/// |---|---|---|
/// | `transcript.rs:998` — live streaming partial | `width - output_pad * 2` | `new Markdown(text, this.outputPad, 0, …)`, `assistant-message.ts:104-114` |
/// | `transcript.rs:2234` — committed assistant turn | `width - output_pad * 2` | same |
/// | `transcript.rs:2196` — user message | `width - output_pad * 2` | `Box(outputPad, 1)`'s `contentWidth` (`box.ts:79`) around a `Markdown(…, 0, 0)`, `user-message.ts:38-58` |
/// | `transcript.rs:1082` — thinking body | **already reduced by its own callers**: `:974` and `:2256` each hand `transcript::thinking_lines` `width - output_pad * 2`, and it forwards that unchanged | `new Markdown(…, outputPad, 0, …, { color, italic })`, `assistant-message.ts:146-164` |
/// | `transcript.rs:2374` — `Entry::Block` | the **full** `width`: the block draws its own edge-to-edge `─` rules and carries no `outputPad` | — |
/// | `transcript.rs:2450` — labeled `[skill]`/custom block | `width - 2` (`:2436` `content_width`), the `Box(1, 1)` `contentWidth` (`box.ts:79`) — **not** `output_pad` | `skill-invocation-message.ts:17` and the three sibling components |
///
/// What every one of them shares — and what this argument actually pins — is that the value is
/// already the CONTENT width. Subtracting padding a second time in here would silently narrow every
/// message by two columns and re-open M9: an assistant `---` drawing 76 where pi draws 78.
///
/// Rows come back already wrapped to `width`: [`MdRenderer::flush_line`] runs
/// `wrapTextWithAnsi(line, contentWidth)` (`markdown.ts:322`, and the narrower `itemWidth` /
/// `quoteContentWidth` of `:788` / `:594` inside a container) and re-applies the row prefix. Nothing
/// downstream needs to reflow them, and reflowing them at a wider width is exactly the L2/M10 bug.
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
    render_inner(text, width, theme, color, italic, crate::image::hyperlinks_supported())
}

/// [`render`] with the terminal's OSC-8 hyperlink capability supplied explicitly instead of read
/// from the process-wide capability cache — Pi's `getCapabilities().hyperlinks` gate on the inline
/// ` (url)` suffix (`tui/src/components/markdown.ts:692-707`).
///
/// Upstream, a hyperlink-capable terminal gets `hyperlink(styledLink, token.href)` (an OSC-8 escape
/// wrapping the link text) and the URL is **not** printed inline; an incapable one gets the legacy
/// `text (url)`. cyrup renders through ratatui's cell buffer, which has no channel for an OSC-8
/// escape — a `\x1b]8;;…` inside a [`Span`] would be laid into cells as literal text — so the
/// capable branch here emits the link text alone, matching upstream's *visible* row exactly while
/// omitting the (unrepresentable) clickable wrapper. The incapable branch is byte-identical to
/// upstream.
///
/// Exists so tests can drive both branches without touching the global cache.
pub fn render_with_hyperlink_support(
    text: &str,
    width: usize,
    theme: &UiTheme,
    hyperlinks: bool,
) -> Vec<Line<'static>> {
    render_inner(text, width, theme, None, false, hyperlinks)
}

fn render_inner(
    text: &str,
    width: usize,
    theme: &UiTheme,
    color: Option<ratatui::style::Color>,
    italic: bool,
    hyperlinks: bool,
) -> Vec<Line<'static>> {
    // Tabs → 3 spaces before parse (`markdown.ts:171`).
    let prepared = text.replace('\t', "   ");
    let mut r = MdRenderer::new(width, theme);
    r.default_text = color;
    r.default_italic = italic;
    r.hyperlinks = hyperlinks;
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    // `into_offset_iter` (rather than the plain event iterator) because two upstream behaviours are
    // defined on the *source* text, not on the event: the strict `~~`-only strikethrough tokenizer
    // (`markdown.ts:7-24`) and the too-narrow table fallback to `token.raw` (`markdown.ts:854-861`).
    for (ev, range) in Parser::new_ext(&prepared, opts).into_offset_iter() {
        let raw = prepared.get(range).unwrap_or("");
        r.event(ev, raw);
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
    /// How many LEADING spans of [`Self::cur`] form this row's `firstPrefix` — the quote borders,
    /// the nesting indent and (when it has been materialised) the list marker
    /// (`markdown.ts:774` `firstPrefix = indent + this.theme.listBullet(marker)`).
    ///
    /// [`MdRenderer::flush_line`] splits there: the prefix stays verbatim on row 0, the remainder is
    /// the BODY that gets wrapped at `width - visibleWidth(firstPrefix)` (`markdown.ts:776`
    /// `itemWidth`), and rows 1..N are re-opened with [`MdRenderer::continuation_prefix`]
    /// (`:789` `renderedAnyLine ? continuationPrefix : firstPrefix`).
    prefix_spans: usize,
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
    /// One frame per *open list item*, innermost last — the state behind Pi's `firstPrefix` /
    /// `continuationPrefix` pair (`markdown.ts:774-775`).
    items: Vec<ItemFrame>,
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
    /// `getCapabilities().hyperlinks` (`markdown.ts:692`): when the terminal forwards OSC-8, Pi
    /// prints the link text ONLY and never the ` (url)` suffix.
    hyperlinks: bool,
    /// One entry per open `Tag::Strikethrough`: `true` when the source delimiter was a **single**
    /// `~`, which Pi's `StrictStrikethroughTokenizer` (`markdown.ts:7-24`) never tokenizes as `del`
    /// — those levels re-emit their literal tildes instead of striking.
    strike_literal: Vec<bool>,
}

/// One open list item's prefix state.
///
/// Upstream builds two prefixes per item and picks between them per emitted row:
/// ```text
/// const firstPrefix = indent + this.theme.listBullet(marker);
/// const continuationPrefix = indent + " ".repeat(visibleWidth(marker));
/// …
/// const linePrefix = renderedAnyLine ? continuationPrefix : firstPrefix;
/// ```
/// (`markdown.ts:774-775`, `:789`). `marker_w` is that `visibleWidth(marker)` — of the WHOLE marker,
/// bullet + task box (`:772-773`) — and `rendered` is `renderedAnyLine`.
#[derive(Default)]
struct ItemFrame {
    /// `visibleWidth(marker)`; 0 until the marker is actually emitted.
    marker_w: usize,
    /// `renderedAnyLine` — false only until this item's first row is opened.
    rendered: bool,
}

#[derive(Default)]
struct TableCapture {
    in_head: bool,
    cur_cell: String,
    cur_row: Vec<String>,
    header: Vec<String>,
    rows: Vec<Vec<String>>,
    /// The table's source Markdown — marked's `token.raw`, the too-narrow fallback body
    /// (`markdown.ts:856`).
    raw: String,
}

impl<'t> MdRenderer<'t> {
    fn new(width: usize, theme: &'t UiTheme) -> Self {
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
            table: None,
            default_text: None,
            default_italic: false,
            hyperlinks: false,
            strike_literal: Vec::new(),
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
        self.open_line();
        if self.link.is_some() {
            self.link_text.push_str(text);
        }
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
            spans.push(Span::styled("│ ".to_string(), self.theme.md_quote_border_style()));
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
    fn content_width(&self) -> usize {
        let mut used: usize = usize::try_from(self.quote).unwrap_or(usize::MAX).saturating_mul(2);
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
    fn flush_line(&mut self) {
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
            let mut out_spans: Vec<Span<'static>> =
                if i == 0 { prefix.to_vec() } else { cont.clone() };
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
    fn emit_prefixed(&mut self, line: Line<'static>) {
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
    fn blank(&mut self) {
        if self.out.last().map(|l| row_is_blank(l, self.quote)).unwrap_or(true) {
            return;
        }
        let mut spans: Vec<Span<'static>> = Vec::new();
        for _ in 0..self.quote {
            spans.push(Span::styled("│ ".to_string(), self.theme.md_quote_border_style()));
        }
        self.out.push(if spans.is_empty() { Line::default() } else { Line::from(spans) });
    }

    /// Emit literal source text through the same three-way sink as [`Event::Text`] (table cell /
    /// code buffer / styled inline run). Used for the tildes of a non-strikethrough `~…~` run.
    fn emit_literal(&mut self, text: &str) {
        if let Some(table) = self.table.as_mut() {
            table.cur_cell.push_str(text);
        } else if self.code_lang.is_some() {
            self.code_buf.push_str(text);
        } else {
            let style = self.inline_style();
            self.push_text(text, style);
        }
    }

    fn event(&mut self, ev: Event<'_>, raw: &str) {
        match ev {
            Event::Start(tag) => self.start(tag, raw),
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
            Event::Html(h) | Event::InlineHtml(h) => {
                let style = self.inline_style();
                self.push_text(h.trim_end_matches('\n'), style);
            }
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
            Tag::List(start) => {
                self.flush_line();
                self.lists.push(start);
            }
            Tag::Item => {
                let marker = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        let s = format!("{n}. ");
                        *n = n.saturating_add(1);
                        s
                    }
                    _ => "- ".to_string(),
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
                self.table = Some(TableCapture { raw: raw.to_string(), ..TableCapture::default() });
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
                while self.out.last().is_some_and(|l| is_quote_only_row(l, self.quote)) {
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
                self.emit_code_block(&lang, &code);
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
        for line in highlight_lines(code, lang, self.theme) {
            self.emit_prefixed(line);
        }
        self.emit_prefixed(Line::styled("```".to_string(), border));
        self.blank();
    }

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
    fn emit_table(&mut self) {
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
                for row in wrap_cell(src, avail_width) {
                    self.push_text(&row, style);
                    self.flush_line();
                }
            }
            // `if (nextTokenType && nextTokenType !== "space") fallbackLines.push("")` (`:857-859`).
            self.blank();
            return;
        }
        let avail_cells = avail_width.saturating_sub(overhead);

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

        // The grid is drawn with NO theme function at all upstream — `` `┌─${…join("─┬─")}─┐` ``
        // (`markdown.ts:956`), `` `│ ${rowParts.join(" │ ")} │` `` (`:971`), `` `├─…─┼─…─┤` ``
        // (`:976`) and `` `└─…─┴─…─┘` `` (`:1003`) are plain template strings, so the frame renders
        // in the same colour as body prose rather than in `mdHr`.
        let base = self.theme.assistant_style();
        // `return this.theme.bold(padded)` (`:966-970`) — pure SGR-1 over the cell's own text; it
        // adds NO foreground of its own, so a header cell is body colour + bold, never `mdHeading`.
        let heading = base.add_modifier(Modifier::BOLD);
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
        self.push_table_row(&t.header, &widths, heading, '│');
        // Separator ├─...─┼─...─┤.
        let sep = || border("├─", "─┼─", "─┤", &widths);
        self.emit_prefixed(sep());
        for (ri, row) in t.rows.iter().enumerate() {
            self.push_table_row(row, &widths, base, '│');
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
    /// the frame; the cell text renders in `cell_style`.
    fn push_table_row(&mut self, cells: &[String], widths: &[usize], cell_style: Style, bar: char) {
        let bar_style = self.theme.assistant_style();
        let wrapped: Vec<Vec<String>> = widths
            .iter()
            .enumerate()
            .map(|(i, w)| wrap_cell(cells.get(i).map(String::as_str).unwrap_or(""), *w))
            .collect();
        let height = wrapped.iter().map(Vec::len).max().unwrap_or(1).max(1);
        for li in 0..height {
            let mut spans: Vec<Span<'static>> = Vec::with_capacity(widths.len() * 2 + 1);
            spans.push(Span::styled(format!("{bar} "), bar_style));
            for (ci, w) in widths.iter().enumerate() {
                if ci > 0 {
                    spans.push(Span::styled(format!(" {bar} "), bar_style));
                }
                let text = wrapped.get(ci).and_then(|c| c.get(li)).cloned().unwrap_or_default();
                let pad = w.saturating_sub(display_width(&text));
                spans.push(Span::styled(format!("{text}{}", " ".repeat(pad)), cell_style));
            }
            spans.push(Span::styled(format!(" {bar}"), bar_style));
            // Through the prefix machinery, never `self.out.push` — see [`Self::emit_table`].
            self.emit_prefixed(Line::from(spans));
        }
    }

    fn finish(mut self) -> Vec<Line<'static>> {
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
        if self.out.last().map(|l| l.spans.iter().all(|s| s.content.trim().is_empty())).unwrap_or(false)
        {
            self.out.pop();
        }
        self.out
    }
}

/// Whether `line` is one of upstream's `""` rows — a row carrying nothing but its container prefix.
///
/// The first `quote` spans of a row emitted inside a blockquote are the `quoteBorder("│ ")` runs
/// `markdown.ts:596` prepends; blankness is decided by what follows them, exactly as upstream tests
/// the *unprefixed* string (`:588`). Skipping them matters both ways: without the skip a `│ `
/// separator reads as content and [`MdRenderer::blank`] emits a second one, and with too generous a
/// skip a table row whose bars happen to be `│ ` would read as blank.
fn row_is_blank(line: &Line<'_>, quote: u32) -> bool {
    let skip = usize::try_from(quote).unwrap_or(usize::MAX);
    line.spans.iter().skip(skip).all(|s| s.content.trim().is_empty())
}

/// Whether `line` is a blank row *produced inside* a blockquote of depth `quote` — exactly `quote`
/// border spans and nothing at all after them.
///
/// This is the test behind `while (renderedQuoteLines.at(-1) === "") renderedQuoteLines.pop()`
/// (`markdown.ts:587-590`). It is deliberately stricter than [`row_is_blank`]: the pop walks
/// BACKWARDS off the end of the output and must not chew into rows that predate the blockquote (a
/// bare `Line::default()` left by the preceding paragraph has zero spans and would otherwise match).
fn is_quote_only_row(line: &Line<'_>, quote: u32) -> bool {
    let want = usize::try_from(quote).unwrap_or(usize::MAX);
    want > 0 && line.spans.len() == want && line.spans.iter().all(|s| s.content.as_ref() == "│ ")
}

/// Strip `depth` levels of blockquote source markers from one line of a table's `token.raw`.
///
/// marked's `blockquote` tokenizer removes the `>` markers before re-lexing the quote body, one
/// level per nesting, so a table nested in a blockquote reaches `renderTable` with a `token.raw`
/// that carries **none** — and `markdown.ts:856`'s `wrapTextWithAnsi(token.raw, availableWidth)`
/// fallback therefore prints clean Markdown. pulldown-cmark's offset range is a slice of the
/// ORIGINAL source, so `Start(Table)`'s raw still has every `> ` on it.
///
/// The shape mirrors marked's `/^ {0,3}> ?/`: up to three leading spaces, the `>`, then one optional
/// space or tab.
fn strip_quote_markers(line: &str, depth: u32) -> &str {
    let mut s = line;
    for _ in 0..depth {
        let mut t = s;
        for _ in 0..3 {
            match t.strip_prefix(' ') {
                Some(rest) => t = rest,
                None => break,
            }
        }
        let Some(rest) = t.strip_prefix('>') else { break };
        s = rest.strip_prefix([' ', '\t']).unwrap_or(rest);
    }
    s
}

/// Visible (terminal-column) width of `s`, unicode-width-correct via ratatui's `Span::width`
/// (matches Pi's `visibleWidth`). Never `str::len`.
fn display_width(s: &str) -> usize {
    Span::raw(s).width()
}

/// Greedy word-wrap `text` to a column of `width` cells — Pi's `wrapCellText` (`markdown.ts:829-831`),
/// which is `wrapTextWithAnsi(text, Math.max(1, maxWidth))`: split on spaces, pack words onto a line,
/// and hard-break any single word wider than the column. Always returns at least one (possibly empty)
/// line so a cell occupies a row.
///
/// The long-word break walks **extended grapheme clusters**, not `char`s — Pi's `breakLongWord`
/// segments with `graphemeSegmenter.segment(textPortion)` and advances one `seg.segment` at a time
/// (`tui/src/utils.ts:977-979`, `:986-1013`). A `char` walk splits a ZWJ emoji family between its
/// members and detaches a combining mark from its base, both of which corrupt the cell.
fn wrap_cell(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    // `if (visibleLength <= width) return [line];` (`utils.ts:1006-1009`) — a line that already fits
    // is returned VERBATIM, keeping its interior spacing rather than being re-packed word by word.
    if display_width(text) <= width {
        return vec![text.to_string()];
    }
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
            for cluster in word.graphemes(true) {
                let cw = display_width(cluster);
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
                chunk.push_str(cluster);
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
