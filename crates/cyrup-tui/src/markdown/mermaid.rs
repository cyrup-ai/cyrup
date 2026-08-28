//! Mermaid fences rendered as Unicode terminal diagrams — the port of
//! `packages/coding-agent/src/modes/interactive/components/mermaid.ts:1-89`.
//!
//! Upstream this is a `MarkdownTransformer`: a markdown-string → markdown-string function
//! registered as a built-in at `interactive-mode.ts:484-486` and applied per `Markdown` component
//! with a `MarkdownTransformContext` (`core/extensions/types.ts:1201-1205`) carrying
//! `messageType`, `isStreaming` and `availableWidth`. cyrup has no transformer registry, so the
//! gate lives here as [`MermaidContext`] and the substitution happens inside the event walk
//! ([`super::walk`]'s `TagEnd::CodeBlock` arm) instead of in a second markdown round-trip.
//!
//! ## Engine decision, 2026-08-28
//! pi calls `render()` from the JS package `grok-mermaid` (`mermaid.ts:2`). The Rust counterpart
//! chosen here is the [`mermaid_text`] crate (MIT), driven source-to-text through
//! [`mermaid_text::render`]. See the workspace `Cargo.toml` for why `mermansi` was rejected after
//! being tried first (it forces `serde_json/preserve_order` on the whole workspace).
//!
//! ## Span classes are INFERRED FROM THE RENDERED GEOMETRY
//! pi themes **six** span classes — `border` -> `borderMuted`, `text` -> `text`, `edge` ->
//! `accent`, `edgeLabel` -> `muted`, `title` -> bold `accent`, `none` -> unstyled (`styleSpan`,
//! `mermaid.ts:38-53`) — reading them off `art.styled: Span[][]`, which its engine `grok-mermaid`
//! reports per cell.
//!
//! [`mermaid_text`] reports nothing of the kind, and there is no accessor to add:
//!
//! * `layout::Grid::fg` is not a class channel. Its own doc
//!   (`mermaid-text-0.57.0/src/layout/grid.rs:286-289`) says the plane is "Empty (all `None`) until
//!   the caller paints colors via `Grid::set_fg` / `paint_fg_rect`", and the painters fire only for
//!   a diagram whose SOURCE carries `style` / `classDef` / `linkStyle`. Where it is non-`None` it
//!   holds the diagram author's chosen RGB, which has no relation to a semantic class.
//! * The `Grid` never escapes the engine. Both public entry points return `String`
//!   (`render/unicode.rs:674`, `:692`) and delegate to the private `render_inner` (`:700`), which
//!   constructs and drops the grid.
//! * It would not generalise anyway: 16 of the 18 `DiagramKind`s early-return a `String` from their
//!   own render module (`lib.rs:283-392`) and never touch that pipeline at all.
//!
//! So [`classify`] derives the six classes from the finished character grid instead — which works
//! for all 18 kinds precisely because it operates on output. **The inference is geometric and it
//! can be wrong.** Known limits, all cosmetic:
//!
//! * A node is recognised by its CLOSED outline (a corner run that meets a matching corner run
//!   below with intact sides). A shape whose corner the engine overwrites with something else —
//!   a class-diagram box terminated by a relationship marker (`└──△──┆`), or one a passing edge
//!   continues straight through (`└────────◆────────│`) — is not recognised at all, so its whole
//!   body falls to the outside rules and its label reads `EdgeLabel` rather than `Text`.
//! * A corner scan alone cannot separate a node from an edge: `DIR_TO_CHAR[0b1010]` is `'┌'`
//!   (`grid.rs:131`), so an edge junction produces the identical glyph. That is why the closed
//!   rectangle match is load-bearing rather than an optimisation — and why the thick set
//!   `┏┓┗┛┃━` is NOT treated as a corner: `THICK_DIR_TO_CHAR` (`grid.rs:79-96`) emits it for UML
//!   fork/join bars, which are edges.
//! * Conversely, an edge route that happens to close a rectangle reads as a node outline
//!   (`Border` instead of `Edge`).
//! * A box-drawing glyph used decoratively **inside** a node label reads as `Border`; a plain-text
//!   label that happens to sit inside a node's bounding box reads as `Text` even when it is an
//!   edge label the router placed there.
//! * `Title` is only ever inferred for the text-report kinds (`pie`, `gantt`, `journey`,
//!   `quadrant`, …) that emit a plain leading line and draw no node box at all —
//!   `render/pie.rs:142-146`, `render/gantt.rs:90-92`. A flowchart never claims one.
//! * Geometry is indexed by DISPLAY column, so a zero-width combining mark inside a label shifts
//!   the columns after it on that row and can misclassify its tail.
//!
//! [`mermaid_text::render`] is used rather than `render_with_width`: the latter compacts the gap
//! configuration to fit a budget, which pi never does. pi measures the finished art and falls back
//! to the raw fence when it does not fit (`mermaid.ts:76`), so the width check below stays ours.
//!
//! ## Not ported, and why
//! pi's `codeSpan` (`mermaid.ts:18-36`) re-encodes every diagram row as an inline code span with a
//! variable-length backtick fence and an NBSP for blank rows, and joins the rows with markdown hard
//! breaks (`:85`). Both exist *solely* because pi's transformer is a markdown-string →
//! markdown-string seam whose output is lexed a second time, so spacing and box-drawing have to
//! survive a re-parse. cyrup's walker emits [`ratatui::text::Line`]s straight into the output, with
//! nothing left to re-parse, so the round-trip has nothing to protect — and skipping it is what
//! lets each row carry its own per-class [`Span`](ratatui::text::Span)s rather than the flat
//! inline-`md_code` role a backtick round-trip would collapse them into. The observable result is
//! the same set of rows, one per line.
//!
//! ## Scope of the hook
//! [`MermaidContext`] is the mermaid-only half of the per-message markdown-transform seam: it
//! reaches exactly the three pi `messageType`s (`user`, `assistant`, `assistant-thinking` —
//! `assistant-message.ts:112` and `:157-161`, `user-message.ts:53`) through
//! [`super::render_message`], and it carries only what the mermaid gate reads.
//!
//! The general, extension-supplied `MarkdownTransformer` registry
//! (`core/extensions/types.ts:1355`) is now wired too, but it does NOT ride this struct: it is
//! applied at push/commit time by `App::apply_markdown_transformers` (`app/events.rs`), because
//! `ExtensionHost::transform_markdown` is `async` while this renderer runs inside `App::draw`. The
//! two share only [`MessageType`] — see [`MessageType::as_pi_str`] and the seam note on
//! [`super::render_message`].

use cyrup_config::MermaidRenderingMode;
use mermaid_text::Error as MermaidError;
// The arrow tips (`▸▾◂▴`) and the non-arrow endpoints (`○ ×`) are `pub` in the engine
// (`layout/grid.rs:44` and `:51`), so they are imported rather than re-typed: an engine bump that
// changes a glyph becomes a compile error here instead of a silent misclassification. The corner
// and side glyphs cannot be — `mod rect` (`:26`), `mod rounded` (`:36`) and `mod dotted` (`:65`)
// are private — so those are spelled as literals below.
use mermaid_text::layout::grid::{arrow, endpoint};

/// Which pi message a markdown body belongs to — `MarkdownTransformContext["messageType"]`
/// (`core/extensions/types.ts:1202`).
///
/// Two consumers read it, and they disagree about which arms matter. The mermaid gate below only
/// branches on `assistant-thinking` (`mermaid.ts:65`); the general, extension-supplied transformer
/// pass (`App::apply_markdown_transformers`) hands all three to the guest verbatim
/// through [`Self::as_pi_str`]. Keeping one discriminant for both is what stops the two from
/// drifting into different spellings of the same three message components.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MessageType {
    User,
    Assistant,
    AssistantThinking,
}

impl MessageType {
    /// The wire spelling of `MarkdownTransformContext["messageType"]`
    /// (`core/extensions/types.ts:1202`) — the literal each of pi's three message components passes
    /// to `createMarkdownTransform`: `"user"` (`user-message.ts:53`), `"assistant"`
    /// (`assistant-message.ts:112`) and `"assistant-thinking"` (`assistant-message.ts:157-161`).
    ///
    /// Guest extensions compare this string, so the three literals live here and nowhere else.
    pub(crate) fn as_pi_str(self) -> &'static str {
        match self {
            MessageType::User => "user",
            MessageType::Assistant => "assistant",
            MessageType::AssistantThinking => "assistant-thinking",
        }
    }
}

/// The three inputs pi's mermaid transformer gates on: the live `markdown.mermaid` mode
/// (`options.getMode()`, `mermaid.ts:62`) plus the two `MarkdownTransformContext` fields it reads
/// (`:65-66`). The third context field, `availableWidth`, is the renderer's own content width and
/// is passed separately to [`render_diagram`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MermaidContext {
    pub mode: MermaidRenderingMode,
    pub message_type: MessageType,
    pub is_streaming: bool,
}

impl MermaidContext {
    /// The pass-through every markdown call site that is not one of pi's three message components
    /// uses: no transformer is attached to those upstream, so a mermaid fence stays a raw fence.
    pub(crate) const OFF: Self = MermaidContext {
        mode: MermaidRenderingMode::Off,
        message_type: MessageType::Assistant,
        is_streaming: false,
    };

    pub(crate) fn new(
        mode: MermaidRenderingMode,
        message_type: MessageType,
        is_streaming: bool,
    ) -> Self {
        MermaidContext { mode, message_type, is_streaming }
    }

    /// pi's gate, inverted (`mermaid.ts:63-70` returns the markdown UNTOUCHED when any of the three
    /// hold): render diagrams unless the mode is `off`, the body is a thinking block, or the body
    /// is streaming while the mode is not `streaming`.
    pub(crate) fn enabled(self) -> bool {
        !(self.mode == MermaidRenderingMode::Off
            || self.message_type == MessageType::AssistantThinking
            || (self.is_streaming && self.mode != MermaidRenderingMode::Streaming))
    }
}

impl Default for MermaidContext {
    fn default() -> Self {
        Self::OFF
    }
}

/// `isMermaid` (`mermaid.ts:14-16`): the fence's info string, trimmed, split on whitespace, first
/// word lowercased, compared to `mermaid`. So ```` ```mermaid title="x" ```` qualifies and
/// ```` ```mermaidish ```` does not.
///
/// `info` is the ALREADY-trimmed info string the `Tag::CodeBlock` arm stored (`walk.rs`), which is
/// the same value marked puts in `token.lang`. `eq_ignore_ascii_case` stands in for pi's
/// `toLowerCase()` because the literal being compared against is pure ASCII, so the two agree on
/// every input (a non-ASCII word cannot lowercase *into* `mermaid`).
pub(crate) fn is_mermaid(info: &str) -> bool {
    info.split_whitespace()
        .next()
        .is_some_and(|word| word.eq_ignore_ascii_case("mermaid"))
}

/// The `markdown.mermaid` row value → [`MermaidRenderingMode`], pi's VALIDATE-not-parse rule
/// (`getMermaidRenderingMode`, settings-manager.ts:1251-1254): `off` and `final` pass, everything
/// else — including an unknown spelling — is `streaming`.
///
/// The same three arms as [`cyrup_config::EffectiveSettings::mermaid_rendering_mode`]
/// (`settings/effective.rs:419-428`); this one exists for the live-apply path, which has only the
/// raw row string in hand and must not wait for the settings layer to be re-read.
pub(crate) fn mode_from_setting(value: &str) -> MermaidRenderingMode {
    match value {
        "off" => MermaidRenderingMode::Off,
        "final" => MermaidRenderingMode::Final,
        _ => MermaidRenderingMode::Streaming,
    }
}

/// pi's span classes — the six arms of `styleSpan` (`mermaid.ts:39-52`), in that order.
///
/// Upstream each is reported by the engine on a `Span`'s `cls`; here they are inferred by
/// [`classify`] from the rendered geometry (see the module doc for where that inference breaks).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SpanClass {
    /// `theme.fg("borderMuted", …)` (`mermaid.ts:40-41`) — a node's outline.
    Border,
    /// `theme.fg("text", …)` (`:42-43`) — a node's label.
    Text,
    /// `theme.fg("accent", …)` (`:44-45`) — link lines, junctions, arrow tips.
    Edge,
    /// `theme.fg("muted", …)` (`:46-47`) — a label riding on a link rather than in a node.
    EdgeLabel,
    /// `theme.fg("accent", theme.bold(…))` (`:48-49`) — the diagram title.
    Title,
    /// `return span.text` (`:50-51`) — unstyled; here, the padding between glyphs.
    None,
}

/// What the walker should emit for one mermaid fence.
pub(crate) enum DiagramOutcome {
    /// The rendered diagram, one entry per row, each row split into per-class runs — the shape of
    /// pi's `art.styled: Span[][]` (`mermaid.ts:55-57` `themedLines`).
    Diagram(Vec<Vec<(SpanClass, String)>>),
    /// `return token.raw` — the untouched fence (`mermaid.ts:74`, `:76`).
    Raw,
    /// The untouched fence followed by this warning line (`mermaid.ts:77-82`).
    Warned(String),
}

/// `Mermaid diagram not rendered: ${art.warnings[0]}${suffix}` with
/// `` suffix = ` (+${n - 1} more)` `` for n > 1 (`mermaid.ts:78-79`). `None` for an empty list,
/// which is upstream's `art.warnings.length > 0` guard at `:77`.
///
/// [`mermaid_text`]'s failure channel is a single typed [`MermaidError`], so today the list this is
/// called with never holds more than one entry and the `(+N more)` leg cannot fire. pi's rule is
/// kept whole anyway — it is the spec, and an engine that grows a warning list later inherits it.
fn warning_line(warnings: &[String]) -> Option<String> {
    let first = warnings.first()?;
    let suffix = match warnings.len() {
        n if n > 1 => format!(" (+{} more)", n - 1),
        _ => String::new(),
    };
    Some(format!("Mermaid diagram not rendered: {first}{suffix}"))
}

/// Top-left corner glyphs a node outline may open with: `rect::TL`, `rounded::TL`
/// (`grid.rs:27`, `:37`) plus the two slanted sides `╱ ╲` the diamond / hexagon / parallelogram /
/// trapezoid shapes use for a corner. The THICK set is deliberately absent — see the module doc.
const TOP_LEFT: [char; 4] = ['┌', '╭', '╱', '╲'];
/// Top-right partners of [`TOP_LEFT`] (`rect::TR`, `rounded::TR`, plus the slanted pair).
const TOP_RIGHT: [char; 4] = ['┐', '╮', '╱', '╲'];
/// Bottom-left closers (`rect::BL`, `rounded::BL`, plus the slanted pair).
const BOTTOM_LEFT: [char; 4] = ['└', '╰', '╱', '╲'];
/// Bottom-right closers (`rect::BR`, `rounded::BR`, plus the slanted pair).
const BOTTOM_RIGHT: [char; 4] = ['┘', '╯', '╱', '╲'];

/// A glyph the engine draws structure with: the whole Unicode **Box Drawing** block (which is
/// every glyph `DIR_TO_CHAR` / `THICK_DIR_TO_CHAR` / `rect` / `rounded` / `dotted` can produce —
/// `grid.rs:26-134`) plus the arrow tips and endpoints the engine exports.
///
/// Block Elements (`█ ░`) are excluded on purpose: they are the pie / gantt bar fills, which are
/// data, not structure.
fn is_structural(ch: char) -> bool {
    matches!(ch, '\u{2500}'..='\u{257F}')
        || ch == arrow::RIGHT
        || ch == arrow::DOWN
        || ch == arrow::LEFT
        || ch == arrow::UP
        || ch == endpoint::CIRCLE
        || ch == endpoint::CROSS
}

/// The class-diagram relationship markers (`△ ◆ ◇` — `render/class.rs:554`, `:558`, `:562` — plus
/// their rotations), which the renderer writes INTO an outline run: `└────────△────────┘`. They are
/// structure for outline-matching purposes even though they live outside the Box Drawing block.
fn is_relationship_marker(ch: char) -> bool {
    matches!(ch, '△' | '▲' | '▽' | '▼' | '◇' | '◆' | '◁' | '◀' | '▷' | '▶')
}

/// A glyph allowed inside the horizontal run between two matching corners.
fn is_border_run(ch: char) -> bool {
    is_structural(ch) || is_relationship_marker(ch)
}

/// A glyph allowed in a node's left or right wall on a row between its corners: the border set
/// plus the ASCII/bracket walls the circle, stadium, hexagon and asymmetric shapes use
/// (`( ) < > ⟨ ⟩`).
fn is_side(ch: char) -> bool {
    is_border_run(ch) || matches!(ch, '(' | ')' | '<' | '>' | '\u{27E8}' | '\u{27E9}')
}

/// A recognised node outline, in DISPLAY columns and row indices, corners inclusive.
#[derive(Clone, Copy)]
struct Rect {
    top: usize,
    bottom: usize,
    left: usize,
    right: usize,
}

impl Rect {
    fn on_outline(&self, row: usize, col: usize) -> bool {
        if row < self.top || row > self.bottom || col < self.left || col > self.right {
            return false;
        }
        row == self.top || row == self.bottom || col == self.left || col == self.right
    }

    fn contains(&self, row: usize, col: usize) -> bool {
        row > self.top && row < self.bottom && col > self.left && col < self.right
    }
}

/// The visible column count of one `char`, routed through the crate's single width primitive.
fn char_cols(ch: char) -> usize {
    let mut buf = [0u8; 4];
    crate::text_width::str_width(ch.encode_utf8(&mut buf))
}

/// The rendered rows re-indexed by DISPLAY column, so geometry lines up the way the engine laid it
/// out (it measures with `unicode-width` too). A double-width glyph owns its first column and
/// leaves `None` in the second; a blank column is `None` as well, which is what makes a run of
/// border glyphs terminate at the first gap.
fn column_grid(rows: &[String]) -> Vec<Vec<Option<char>>> {
    rows.iter()
        .map(|row| {
            let mut line: Vec<Option<char>> = Vec::new();
            for ch in row.chars() {
                line.push(if ch == ' ' { None } else { Some(ch) });
                for _ in 1..char_cols(ch).max(1) {
                    line.push(None);
                }
            }
            line
        })
        .collect()
}

fn cell(grid: &[Vec<Option<char>>], row: usize, col: usize) -> Option<char> {
    grid.get(row)?.get(col).copied().flatten()
}

/// Every closed outline in the grid. Overlaps are kept (a double-circle nests one inside another);
/// the classifier only ever asks "is this cell on/inside ANY of them".
fn find_rects(grid: &[Vec<Option<char>>]) -> Vec<Rect> {
    let mut rects = Vec::new();
    for (row, line) in grid.iter().enumerate() {
        for (col, slot) in line.iter().enumerate() {
            if slot.is_some_and(|ch| TOP_LEFT.contains(&ch))
                && let Some(rect) = rect_from(grid, row, col)
            {
                rects.push(rect);
            }
        }
    }
    rects
}

/// Walk right from a candidate top-left corner, and at every candidate top-right partner try to
/// close the box below. The FIRST partner that closes wins, which is what keeps
/// `┌────────┐──────┐` — a box whose top edge an edge route continues past — bound to its own
/// corner rather than to the junction six columns further on.
fn rect_from(grid: &[Vec<Option<char>>], top: usize, left: usize) -> Option<Rect> {
    let width = grid.get(top)?.len();
    for right in left.saturating_add(1)..width {
        let ch = cell(grid, top, right)?;
        if TOP_RIGHT.contains(&ch)
            && let Some(bottom) = find_bottom(grid, top, left, right)
        {
            return Some(Rect { top, bottom, left, right });
        }
        if !is_border_run(ch) {
            return None;
        }
    }
    None
}

/// The first row below `top` that closes the two walls with a matching corner pair over an
/// unbroken border run. Bails as soon as either wall stops being a wall.
fn find_bottom(grid: &[Vec<Option<char>>], top: usize, left: usize, right: usize) -> Option<usize> {
    for row in top.saturating_add(1)..grid.len() {
        let (Some(l), Some(r)) = (cell(grid, row, left), cell(grid, row, right)) else {
            return None;
        };
        if BOTTOM_LEFT.contains(&l)
            && BOTTOM_RIGHT.contains(&r)
            && (left.saturating_add(1)..right)
                .all(|c| cell(grid, row, c).is_some_and(is_border_run))
        {
            return Some(row);
        }
        if !(is_side(l) && is_side(r)) {
            return None;
        }
    }
    None
}

/// The row index carrying the diagram title, if one is inferable.
///
/// Only the text-report kinds emit one, and they emit it as a plain leading line with no box
/// anywhere in the drawing (`render/pie.rs:142-146` centres it and follows it with a blank;
/// `render/gantt.rs:90-92` puts it first). Requiring BOTH "no node outline anywhere" and "row 0 has
/// no structural glyph" is what stops a flowchart's first edge-label row from claiming the class.
fn title_row(rows: &[String], rects: &[Rect]) -> Option<usize> {
    if !rects.is_empty() {
        return None;
    }
    let first = rows.first()?;
    if first.trim().is_empty() || first.chars().any(is_border_run) {
        return None;
    }
    Some(0)
}

/// Split the rendered rows into pi's six span classes, one `(class, text)` run per style change —
/// the shape `themedLines` consumes (`mermaid.ts:55-57`).
///
/// The classes are inferred, not reported; the module doc lists where the inference breaks.
fn classify(rows: &[String]) -> Vec<Vec<(SpanClass, String)>> {
    let grid = column_grid(rows);
    let rects = find_rects(&grid);
    let title = title_row(rows, &rects);
    // A drawing with no node outline at all is a text report (pie, gantt, journey, quadrant …):
    // its prose is the diagram's own content, so it reads `Text`. In a drawing that HAS nodes, the
    // prose outside them is by construction a label riding on a link — pi's `edgeLabel`.
    let text_report = rects.is_empty();

    rows.iter()
        .enumerate()
        .map(|(row, line)| {
            let mut runs: Vec<(SpanClass, String)> = Vec::new();
            let mut col = 0usize;
            for ch in line.chars() {
                let class = class_of(ch, row, col, &rects, title == Some(row), text_report);
                match runs.last_mut() {
                    Some((prev, text)) if *prev == class => text.push(ch),
                    _ => runs.push((class, ch.to_string())),
                }
                col = col.saturating_add(char_cols(ch).max(1));
            }
            runs
        })
        .collect()
}

fn class_of(
    ch: char,
    row: usize,
    col: usize,
    rects: &[Rect],
    in_title_row: bool,
    text_report: bool,
) -> SpanClass {
    if rects.iter().any(|r| r.on_outline(row, col)) {
        return SpanClass::Border;
    }
    if rects.iter().any(|r| r.contains(row, col)) {
        // A structural glyph strictly inside an outline is the engine's own chrome — the cylinder's
        // lid rule, the class-diagram compartment separator `├──┤`, the inner ring of a double
        // circle — so it stays `Border` rather than becoming label text.
        return if ch == ' ' {
            SpanClass::None
        } else if is_border_run(ch) {
            SpanClass::Border
        } else {
            SpanClass::Text
        };
    }
    if ch == ' ' {
        return SpanClass::None;
    }
    if is_border_run(ch) {
        return SpanClass::Edge;
    }
    if in_title_row {
        return SpanClass::Title;
    }
    if text_report { SpanClass::Text } else { SpanClass::EdgeLabel }
}

/// Render one mermaid fence body, applying pi's width and warning rules (`mermaid.ts:75-82`).
///
/// `available_width` is `context.availableWidth` — the renderer's content width
/// (`markdown.ts:284`), which is why the walker only takes this path for a TOP-LEVEL fence.
pub(crate) fn render_diagram(
    source: &str,
    available_width: usize,
    is_streaming: bool,
) -> DiagramOutcome {
    // pulldown-cmark's code-block text carries the newline before the closing fence; marked's
    // `token.text` does not (see `emit_fence_rows`' note on the same strip). Feed the engine what
    // pi feeds `render(token.text)`.
    let source = source.strip_suffix('\n').unwrap_or(source);

    // pi feeds `render(token.text)` with no width budget and no options: `grok-mermaid` lays the
    // art out at its natural size and pi compares `art.width` against the pane afterwards
    // (`mermaid.ts:76`). `render_with_width` would instead compact the gap configuration to fit,
    // which changes the drawing rather than falling back — so the plain entry point is the
    // faithful one and the width check stays below.
    let text = match mermaid_text::render(source) {
        Ok(text) => text,
        // A typed error tells us WHY, which lets this match pi's two distinct behaviours instead
        // of guessing from a message string (the previous engine needed magic-string sentinels
        // here). `EmptyInput` and `UnsupportedDiagram` are "there is nothing to draw" — pi's
        // `if (!art || ...) return token.raw` (`:76`) — so the fence prints untouched and silently.
        // A `ParseError` is a real diagnostic and is what pi surfaces as a warning line.
        Err(MermaidError::EmptyInput | MermaidError::UnsupportedDiagram(_)) => {
            return DiagramOutcome::Raw;
        }
        // `if (!context.isStreaming && art.warnings.length > 0)` (`:77`): while streaming, pi never
        // surfaces a warning — it renders whatever came back. A failure here has no diagram to
        // render, so the streaming leg falls through to the untouched fence.
        Err(e) => {
            if is_streaming {
                return DiagramOutcome::Raw;
            }
            return match warning_line(&[e.to_string()]) {
                Some(line) => DiagramOutcome::Warned(line),
                None => DiagramOutcome::Raw,
            };
        }
    };

    let rows: Vec<String> = text.trim_end_matches('\n').lines().map(str::to_string).collect();
    // `if (!art || …) return token.raw` (`:76`) — nothing to draw.
    if rows.is_empty() || rows.iter().all(|row| row.trim().is_empty()) {
        return DiagramOutcome::Raw;
    }
    // `art.width > context.availableWidth` (`:76`). `mermaid_text` returns text rather than an
    // `art` with a `width`, so the width is measured off the rendered rows in display columns —
    // the same unit the pane width is in.
    let width = rows.iter().map(|row| crate::text_width::str_width(row)).max().unwrap_or(0);
    if width > available_width {
        return DiagramOutcome::Raw;
    }
    // `const lines = options.theme ? themedLines(art, options.theme) : art.plain` (`mermaid.ts:83`)
    // — the theme is always present here, so the rows leave classified.
    DiagramOutcome::Diagram(classify(&rows))
}
