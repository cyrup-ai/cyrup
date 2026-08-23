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

use crate::text_width::spans_width;
use crate::theme::UiTheme;
use crate::transcript::is_ws_grapheme;

mod highlight;

/// M12 — `renderLatex`, the port of `tui/src/latex.ts` that `markdown.ts:505-512` and `:645-652`
/// call. A child module of `markdown` because it has exactly one consumer, matching upstream's own
/// `import { renderLatex } from "../latex.ts"` at `markdown.ts:2`.
mod latex;
mod prepass;
mod table;
mod walk;

pub use prepass::trim_partial_closing_fence;

pub(crate) use highlight::highlight_code_lines;

// The markdown-internal items the submodules share. Re-bound here so every submodule reaches them
// through its own `use super::*;`, the same way `crate::transcript`'s split modules do.
use highlight::highlight_lines;
use prepass::{latex_prepass, MATH_END, MATH_START};
use table::trim_cell;

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
/// | `transcript.rs` — `Entry::Block` | `width - 2` | `new Markdown(body, 1, 1, …)` — `paddingX` 1, so `contentWidth = width - 2` (`markdown.ts:284`); the third argument is `paddingY`, not a left margin (`markdown.ts:250-260`). Only the block's two `─` rules span the full `width`. `/changelog` interactive-mode.ts:6071, `/hotkeys` :6202 |
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
    // M12 — run the LaTeX tokenizer extensions BEFORE the markdown parse.
    let (prepared, math) = latex_prepass(&prepared);
    let mut r = MdRenderer::new(width, theme);
    r.math = math;
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
    /// M12 — rendered LaTeX per placeholder index, pre-split on `\n`. `latexBlock` pushes one output
    /// line per row (`markdown.ts:511-513`), so the rows are kept apart rather than re-joined.
    math: Vec<Vec<String>>,
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

/// One table cell: the STYLED inline run upstream's `renderInlineTokens(cell.tokens, styleContext)`
/// produces (`markdown.ts:869`, `:875`, `:960`, `:983`).
///
/// M7: a cell is not plain text. Upstream runs the identical inline renderer over a cell that it
/// runs over a paragraph, so `**bold**`, `` `code` ``, `[a](b)`, `~~del~~` and `*em*` all keep their
/// styling inside the grid — and the widths at `:870`/`:876` are `visibleWidth()` of that styled
/// string, i.e. the VISIBLE width, ANSI excluded. Capturing cells as `String` dropped every one of
/// those styles on the floor.
type CellSpans = Vec<Span<'static>>;

#[derive(Default)]
struct TableCapture {
    in_head: bool,
    cur_cell: CellSpans,
    cur_row: Vec<CellSpans>,
    header: Vec<CellSpans>,
    rows: Vec<Vec<CellSpans>>,
    /// The table's source Markdown — marked's `token.raw`, the too-narrow fallback body
    /// (`markdown.ts:856`).
    raw: String,
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
