//! The display inputs a registered renderer is invoked under (EXT-006).
//!
//! Upstream every renderer signature is `(payload, options, theme)`, and both halves are LIVE:
//! `pi` re-invokes the renderer from the draw path, so a `Ctrl+O` toggle or a `/theme` switch
//! reaches a component that was pushed under the old values.
//!
//! * `MessageRenderer = (message: CustomMessage<T>, options: MessageRenderOptions, theme: Theme)
//!   => Component | undefined` — `pi/packages/coding-agent/src/core/extensions/types.ts:1213-1217`
//!   @v0.84.4, with `MessageRenderOptions { expanded, outputPad }` at `:1195-1199`;
//! * `EntryRenderer = (entry, options: EntryRenderOptions, theme)` — `:1219-1223`, with
//!   `EntryRenderOptions { expanded }` at `:1209-1211`;
//! * `ToolDefinition.renderCall = (args, theme, context)` — `:491` — and
//!   `renderResult = (result, options: ToolRenderResultOptions, theme, context)` — `:493-498` —
//!   with `ToolRenderResultOptions { expanded, isPartial }` at `:413-418`.
//!
//! **Tag-to-tag drift.** At the ported baseline v0.83.0 the message surface took the SAME
//! `EntryRenderOptions { expanded }` the entry surface takes; `MessageRenderOptions` and its
//! `outputPad` field are a post-baseline addition. Per ADR-0006 the parity target is the latest
//! tag, so `output_pad` below is carried.
//!
//! cyrup's WIT keeps ONE renderer pair (`render-call`/`render-result`) and tells the three surfaces
//! apart by the host's registry table rather than by the wire shape
//! ([`crate::ExtensionHost::render_message_call`]), so [`RenderOptions`] is the UNION of the three
//! option bags above: a surface that has no meaning for a field simply leaves it at its default.

use serde::{Deserialize, Serialize};

/// The `(options, theme)` half of upstream's renderer signature — see the module docs for the three
/// upstream bags this is the union of.
///
/// # CYRUP-DELTA: the theme crosses as a NAME
/// Upstream hands the renderer the live `Theme` OBJECT and the renderer calls `theme.fg(role, text)`
/// on it. A `wasmtime` guest cannot receive an object, so the wire carries the theme's NAME — the
/// identity that changes when the user switches themes — and a guest that wants the palette reads
/// it with the `ui.theme-get-json` import, which exists for exactly this and returns the ACTIVE
/// theme (EXT-066, `crates/cyrup-ext/wit/world.wit`). A NATIVE renderer has no such constraint and
/// receives the real thing: [`crate::RenderCtx::theme`] is a `&dyn `[`crate::RenderTheme`].
///
/// # Why this is a value, not a handle
/// It is compared for equality by the consumer that must decide whether already-rendered text is
/// STALE (`cyrup_tui::transcript::RenderSource`), which is what makes the re-invocation derivable
/// from the data instead of remembered by whoever toggles a flag.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RenderOptions {
    /// `options.expanded` on all three upstream bags — the live `app.tools.expand` flag
    /// (`interactive-mode.ts:4032-4048` re-broadcasts it to every child on every toggle, and
    /// `:3437` seeds it into a freshly added `CustomEntryComponent`).
    pub expanded: bool,
    /// `MessageRenderOptions.outputPad` (`types.ts:1198` @v0.84.4) — "Horizontal padding configured
    /// by the outputPad setting". Zero on the surfaces whose bag does not carry it.
    pub output_pad: u32,
    /// `ToolRenderResultOptions.isPartial` (`types.ts:417` @v0.84.4) — whether the result being
    /// rendered is a partial/streaming one. `false` on the surfaces whose bag does not carry it.
    pub is_partial: bool,
    /// The name of the theme the renderer should draw for — see the CYRUP-DELTA above. `None` when
    /// the caller has no display (an RPC host, a test) and therefore no active theme to name.
    pub theme: Option<String>,
}

impl RenderOptions {
    /// The options an interactive display renders under.
    pub fn new(expanded: bool, output_pad: u32, theme: Option<String>) -> Self {
        Self {
            expanded,
            output_pad,
            is_partial: false,
            theme,
        }
    }

    /// [`Self::new`] for the tool-RESULT surface, the only one whose upstream bag carries
    /// `isPartial` (`ToolRenderResultOptions`, `types.ts:413-418` @v0.84.4).
    #[must_use]
    pub fn partial(mut self, is_partial: bool) -> Self {
        self.is_partial = is_partial;
        self
    }

    /// The JSON the guest export receives as `opts-json`, in upstream's field spelling.
    ///
    /// Infallible: every field is a bool, a `u32` or a `String`, so `to_string` cannot fail; the
    /// empty object is the shape-preserving last resort rather than a panic the workspace lints
    /// forbid.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// Parse what [`Self::to_json`] wrote. An absent or malformed field takes its default rather
    /// than failing the render — a renderer that cannot be told the options must still draw, which
    /// is upstream's behaviour for a renderer whose `options` argument it never reads.
    pub fn from_json(s: &str) -> Self {
        serde_json::from_str(s).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::RenderOptions;

    #[test]
    fn the_wire_spelling_is_upstreams() {
        let opts = RenderOptions::new(true, 2, Some("dark".into())).partial(true);
        let json = opts.to_json();
        assert!(json.contains("\"expanded\":true"), "{json}");
        assert!(json.contains("\"outputPad\":2"), "{json}");
        assert!(json.contains("\"isPartial\":true"), "{json}");
        assert!(json.contains("\"theme\":\"dark\""), "{json}");
        assert_eq!(RenderOptions::from_json(&json), opts);
    }

    #[test]
    fn a_missing_or_malformed_bag_takes_the_defaults_rather_than_failing_the_render() {
        assert_eq!(RenderOptions::from_json("{}"), RenderOptions::default());
        assert_eq!(
            RenderOptions::from_json("not json"),
            RenderOptions::default()
        );
        assert_eq!(
            RenderOptions::from_json("{\"expanded\":true}"),
            RenderOptions {
                expanded: true,
                ..RenderOptions::default()
            }
        );
    }
}
