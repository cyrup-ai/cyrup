//! Component trees a NATIVE tool renderer hands the host to lay out (pi `Component`s returned from
//! `ToolDefinition.renderCall` / `renderResult`, `extensions/types.ts`).
//!
//! # Why a tree and not rows
//! [`crate::RenderedComponent`] returns finished rows, so the component has to wrap text to the
//! terminal width itself. A tool renderer's output is not plain rows: pi composes `Container`,
//! `Text`, `Spacer` and `VisualLinePreview` (`modes/interactive/components/visual-truncate.ts`),
//! whose job is to wrap **styled** text to the width in force and keep N *wrapped* lines. Wrapping
//! styled text is the host's one implementation (the transcript's `wrap_line`); a renderer that had
//! to bring its own would be a second one. The renderer therefore describes the tree and the host
//! lays it out, per frame, at the live width.
//!
//! The tree borrows the theme it was built from (a [`RenderNode::VisualLinePreview`] hint is a
//! closure over it), so a tree lives for exactly one layout pass and is rebuilt on the next frame,
//! which is what keeps a theme switch and the expand toggle live.

use crate::native::RenderTheme;

/// Which end of a [`RenderNode::VisualLinePreview`] survives truncation (`keep: "start" | "end"`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewKeep {
    /// Keep the first lines; the hint follows them.
    Start,
    /// Keep the last lines; the hint precedes them.
    End,
}

/// One node of a tool renderer's component tree.
///
/// Text is SGR-styled (the strings [`RenderTheme::fg`] returns); the host converts it back to styled
/// cells.
pub enum RenderNode<'a> {
    /// pi `new Text(text, 0, 0)`: the text wrapped to the width, `\n`-separated lines kept, nothing
    /// at all when the text is blank.
    Text(String),
    /// pi `new Spacer(n)`: `n` blank rows.
    Spacer(usize),
    /// pi `Container`: the children stacked.
    Container(Vec<RenderNode<'a>>),
    /// pi `VisualLinePreview({ text, maxVisualLines, keep, formatHint })`: `text` wrapped to the
    /// width and cut to `max_visual_lines` WRAPPED lines, with `hint(hidden)` marking the cut.
    VisualLinePreview {
        text: String,
        max_visual_lines: usize,
        keep: PreviewKeep,
        hint: Box<dyn Fn(usize) -> String + 'a>,
    },
}

/// What a tree is built under — the options half of pi's `(options, theme)` pair that is live per
/// frame. The width is not here: the host owns layout.
pub struct TreeCtx<'a> {
    /// `options.expanded` / `context.expanded` — the live expand flag of THIS row.
    pub expanded: bool,
    /// pi `theme`.
    pub theme: &'a dyn RenderTheme,
}

/// A tool renderer's retained component (the value pi's `renderCall` / `renderResult` return and
/// `ToolExecutionComponent` keeps as `callRendererComponent` / `resultRendererComponent`).
///
/// It captures what pi's renderer closed over at call time (the arguments, the result, `isPartial`,
/// `isError`) and is asked for its tree on EVERY frame, so it must be cheap and must not panic.
pub trait RenderedTree: std::fmt::Debug + Send + Sync {
    /// The tree to draw now. Owns its text; borrows only the theme in `ctx`.
    fn tree<'a>(&self, ctx: &TreeCtx<'a>) -> RenderNode<'a>;
}
