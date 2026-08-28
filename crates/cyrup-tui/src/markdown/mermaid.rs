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
//! **The diagram is rendered COLOURLESS and styled with the single `md_code_block` role.** pi
//! themes four span classes — `border` -> `borderMuted`, `edge` -> `accent`, `edgeLabel` ->
//! `muted`, `title` -> bold `accent` (`mermaid.ts:38-56`). That fidelity is deferred, NOT
//! unreachable: `mermaid_text` renders into a `layout::Grid` that keeps per-cell structure
//! (`cells: Vec<Vec<char>>` beside `fg: Vec<Vec<Option<Rgb>>>`), `Grid` is public and
//! `Grid::get(col, row) -> char` with it. The one missing piece is a `get_fg` accessor — `fg` is
//! private and escapes only through `Grid::render_with_colors`, which bakes ANSI. A ~5-line
//! upstream PR adding that accessor, plus a grid walk here mapping `Rgb` onto [`crate::theme`]
//! roles, buys the full four-class theming with NO ANSI parser. Until then the diagram — which is
//! the feature — renders in one role.
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
//! lets the rows carry the `md_code_block` role the engine decision calls for rather than the
//! inline-`md_code` role a backtick round-trip would produce. The observable result is the same set
//! of rows, one per line.
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

/// What the walker should emit for one mermaid fence.
pub(crate) enum DiagramOutcome {
    /// The rendered diagram, one entry per row.
    Diagram(Vec<String>),
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
    DiagramOutcome::Diagram(rows)
}
