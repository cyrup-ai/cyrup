//! The serialized widget tree a guest renderer returns (EXT-006; `wit/world.wit`
//! `render-call`/`render-result`).
//!
//! Pi's `ToolDefinition.renderCall`/`renderResult` and `registerMessageRenderer` return a live
//! `pi-tui` `Component`, which the interactive mode adds as a child of the tool row /
//! `CustomMessageComponent` (`components/tool-execution.ts:81-112`,
//! `components/custom-message.ts:66-81`). A WASM guest cannot hand an object across the component
//! boundary, so cyrup's analog is that tree SERIALIZED — and the host is what turns it back into
//! rows (`cyrup-tui` `rendered_text`).
//!
//! These constructors are the authoritative producer of that vocabulary: use them instead of
//! hand-writing `json!({...})`, because the host draws anything it does not recognize as
//! pretty-printed JSON. Each maps to the `pi-tui` component named on its doc line
//! (`packages/tui/src/index.ts:13-32`).
//!
//! ```
//! use cyrup_ext_sdk::widget;
//! let tree = widget::stack([
//!     widget::text("read src/main.rs"),
//!     widget::spacer(1),
//!     widget::markdown("**42** lines"),
//! ]);
//! ```

use serde_json::{Value, json};

/// `Text` — a single (possibly multi-line) run of text. The shape virtually every Pi renderer
/// returns (`new Text(str, 0, 0)`).
pub fn text(s: impl Into<String>) -> Value {
    json!({ "widget": "text", "text": s.into() })
}

/// `Markdown` — text the host renders as markdown.
pub fn markdown(s: impl Into<String>) -> Value {
    json!({ "widget": "markdown", "text": s.into() })
}

/// `TruncatedText` — text the host may elide to the available width.
pub fn truncated_text(s: impl Into<String>) -> Value {
    json!({ "widget": "truncated-text", "text": s.into() })
}

/// `Spacer` — `lines` blank rows (clamped host-side to 64).
pub fn spacer(lines: u32) -> Value {
    json!({ "widget": "spacer", "lines": lines })
}

/// `Container` — children stacked vertically, one after another.
pub fn stack(children: impl IntoIterator<Item = Value>) -> Value {
    json!({ "widget": "container", "children": children.into_iter().collect::<Vec<_>>() })
}

/// `Box` — children stacked vertically inside the host's padded/backed frame.
pub fn boxed(children: impl IntoIterator<Item = Value>) -> Value {
    json!({ "widget": "box", "children": children.into_iter().collect::<Vec<_>>() })
}

/// `HStack` — children joined side by side on ONE row.
pub fn hstack(children: impl IntoIterator<Item = Value>) -> Value {
    json!({ "widget": "hstack", "children": children.into_iter().collect::<Vec<_>>() })
}

/// Where an extension widget is drawn (Pi `WidgetPlacement = "aboveEditor" | "belowEditor"`,
/// `extensions/types.ts:104` @v0.83.0). Passed to [`crate::ctx::Ui::set_widget`] through
/// `ExtensionWidgetOptions.placement` (`:107-110`), whose documented default is `AboveEditor`.
///
/// EXT-047: before the WIT carried pi's three `setWidget` arguments, placement was unexpressible —
/// every extension widget landed in the one slot cyrup had.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WidgetPlacement {
    /// `"aboveEditor"` — upstream's documented default for
    /// `ExtensionWidgetOptions.placement`, and the [`Default`] here.
    #[default]
    AboveEditor,
    /// `"belowEditor"`.
    BelowEditor,
}

impl WidgetPlacement {
    /// The upstream wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AboveEditor => "aboveEditor",
            Self::BelowEditor => "belowEditor",
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]
    use super::*;

    /// The tags are a WIRE CONTRACT with `cyrup-tui`'s flattener — pin them so a rename here that
    /// the host does not learn about fails on this side too, rather than silently degrading a
    /// guest's rendering to a JSON blob on screen.
    #[test]
    fn the_constructors_emit_the_documented_tags() {
        assert_eq!(text("hi"), json!({ "widget": "text", "text": "hi" }));
        assert_eq!(
            markdown("**b**"),
            json!({ "widget": "markdown", "text": "**b**" })
        );
        assert_eq!(
            truncated_text("long"),
            json!({ "widget": "truncated-text", "text": "long" })
        );
        assert_eq!(spacer(2), json!({ "widget": "spacer", "lines": 2 }));
        assert_eq!(
            stack([text("a"), text("b")]),
            json!({ "widget": "container", "children": [
                { "widget": "text", "text": "a" },
                { "widget": "text", "text": "b" },
            ]})
        );
        assert_eq!(boxed([])["widget"], json!("box"));
        assert_eq!(hstack([])["widget"], json!("hstack"));
    }

    /// EXT-047 — the placement strings are pi's literal union members
    /// (`extensions/types.ts:104` @v0.83.0) and cross the WIT as `opts-json`, so a typo here is a
    /// silently-ignored placement host-side (`WidgetPlacement::from_opts` falls back to the
    /// default). Pin them.
    #[test]
    fn placement_spells_pis_union_members() {
        assert_eq!(WidgetPlacement::AboveEditor.as_str(), "aboveEditor");
        assert_eq!(WidgetPlacement::BelowEditor.as_str(), "belowEditor");
        assert_eq!(WidgetPlacement::default(), WidgetPlacement::AboveEditor);
    }
}
