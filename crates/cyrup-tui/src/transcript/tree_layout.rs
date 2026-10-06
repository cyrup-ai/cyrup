//! Layout of a native tool renderer's component tree ([`cyrup_ext::RenderNode`]) into styled rows —
//! the host half of pi's `Container`, `Text`, `Spacer` and `VisualLinePreview`
//! (`packages/tui/src/components/{text,spacer}.ts`, `visual-truncate.ts`).
//!
//! The renderer describes the tree; this wraps it at the width in force with the one wrapper the
//! rest of the transcript uses ([`wrap_line`], via [`text_lines_of`]), so styled text breaks exactly
//! as every built-in row's does.

use super::*;

use cyrup_ext::{PreviewKeep, RenderNode, RenderedTree, TreeCtx};

/// `new Text(text, 0, 0).render(width)`: nothing for blank text, otherwise each `\n`-separated line
/// (`wrapTextWithAnsi` splits on `\r\n|\r|\n` first, `utils.ts:839`) wrapped to the width with its
/// SGR runs carried across the breaks.
fn text_rows(text: &str, width: usize) -> Vec<Line<'static>> {
    if text.trim().is_empty() {
        return Vec::new();
    }
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = Vec::new();
    for logical in normalized.split('\n') {
        let logical = normalize_terminal_output(logical).into_owned();
        out.extend(text_lines_of(&crate::ansi::sgr_line(&logical), width, 0));
    }
    out
}

/// The rows of one node at `width`.
fn node_rows(node: &RenderNode<'_>, width: usize) -> Vec<Line<'static>> {
    match node {
        RenderNode::Text(text) => text_rows(text, width),
        RenderNode::Spacer(rows) => vec![Line::default(); *rows],
        RenderNode::Container(children) => children
            .iter()
            .flat_map(|child| node_rows(child, width))
            .collect(),
        RenderNode::VisualLinePreview {
            text,
            max_visual_lines,
            keep,
            hint,
        } => {
            let rows = text_rows(text, width);
            let hidden = rows.len().saturating_sub(*max_visual_lines);
            if hidden == 0 {
                return rows;
            }
            let hint_rows = text_rows(&hint(hidden), width);
            match keep {
                PreviewKeep::Start => rows
                    .into_iter()
                    .take(*max_visual_lines)
                    .chain(hint_rows)
                    .collect(),
                PreviewKeep::End => hint_rows
                    .into_iter()
                    .chain(rows.into_iter().skip(hidden))
                    .collect(),
            }
        }
    }
}

/// Lay `tree` out at `width` under the live `expanded` flag and theme: pi's `component.render(width)`
/// for a tool renderer's component, run on every paint.
pub(super) fn tree_lines(
    tree: &dyn RenderedTree,
    expanded: bool,
    width: usize,
    theme: &UiTheme,
    expand_key: &str,
) -> Vec<Line<'static>> {
    let roles = crate::theme::UiThemeRoles::new(theme).with_expand_key(expand_key);
    let ctx = TreeCtx {
        expanded,
        theme: &roles,
    };
    node_rows(&tree.tree(&ctx), width.max(1))
}
