//! The demo extension's RENDERERS: the four renderer types and the four registrations that key
//! them — two message renderers (the custom type `demo`, and the per-tool renderer for
//! `demo_echo`) and two entry renderers (`demo_card`, which draws, and `demo_boom`, which
//! deliberately faults).

use crate::{ExtensionApi, MessageRenderer, RenderOptions};
use serde_json::Value;

/// A trivial custom renderer for the demo's `custom_type` (Pi `renderCall`/`renderResult`).
///
/// The return is a SERIALIZED WIDGET TREE — cyrup's wire analog of the `pi-tui` `Component` a Pi
/// renderer returns. Build it with [`crate::widget`] rather than a hand-written `json!`: the host
/// draws only the documented vocabulary as rows and falls back to pretty-printed JSON for anything
/// else, so a typo here is a JSON blob in the user's transcript.
struct DemoRenderer;
impl MessageRenderer for DemoRenderer {
    fn render_call(&self, call: &Value, _opts: &RenderOptions, _ctx: &crate::Ctx) -> Option<Value> {
        Some(crate::widget::text(format!("demo call: {call}")))
    }
    fn render_result(
        &self,
        result: &Value,
        _opts: &RenderOptions,
        _ctx: &crate::Ctx,
    ) -> Option<Value> {
        Some(crate::widget::text(format!("demo result: {result}")))
    }
}

/// The per-TOOL renderer for `demo_echo` (Pi `ToolDefinition.renderCall`/`renderResult`,
/// extensions/types.ts:489-497). Registered under the TOOL NAME, which is the key the host routes
/// a tool row by (EXT-006).
///
/// The call side returns a MULTI-NODE tree (Pi renderers routinely return a `Container` of a header
/// plus detail rows) so the demo exercises the host flattener's stacking, not just the degenerate
/// single-`Text` case.
struct DemoToolRenderer;
impl MessageRenderer for DemoToolRenderer {
    fn render_call(&self, call: &Value, _opts: &RenderOptions, _ctx: &crate::Ctx) -> Option<Value> {
        Some(crate::widget::stack([
            crate::widget::text(format!("guest-rendered echo call: {call}")),
            crate::widget::text("(drawn by the demo extension)"),
        ]))
    }
    /// EXT-006 — the RESULT side branches on `opts.expanded`, which is what a pi renderer does
    /// (`ToolRenderResultOptions.expanded`, `extensions/types.ts:415` @v0.84.4) and what the
    /// collapsed/expanded forms of every built-in tool row are. It is also the fixture that proves
    /// the host re-invokes the renderer on a toggle rather than serving frozen text: the two forms
    /// differ, so a stale render is visible.
    fn render_result(
        &self,
        result: &Value,
        opts: &RenderOptions,
        _ctx: &crate::Ctx,
    ) -> Option<Value> {
        Some(crate::widget::text(if opts.expanded {
            format!("guest-rendered echo result (expanded): {result}")
        } else {
            "guest-rendered echo result (collapsed)".to_string()
        }))
    }
}

/// A custom-ENTRY renderer (Pi `registerEntryRenderer`, `extensions/types.ts:1279` @v0.83.0; EXT-036 corrected `:1295`, which is `sendUserMessage` at that tag — `:1295` is this symbol at v0.84.1, so the old cite was version lag, not fabrication). Entries are
/// TUI-only durable state appended with `append_entry`; they never enter LLM context. An entry
/// crosses the boundary on `render-call`, so the renderer only implements that half.
struct DemoEntryRenderer;
impl MessageRenderer for DemoEntryRenderer {
    /// The theme half of EXT-006's fixture: pi hands every renderer the live `Theme` and cyrup
    /// hands it the theme's NAME (an object cannot cross the component boundary — a guest that
    /// needs the palette calls `ui.theme_get_json()`, EXT-066). Naming it in the output is what
    /// lets a test see that a `/theme` switch re-invoked the renderer.
    fn render_call(&self, entry: &Value, opts: &RenderOptions, _ctx: &crate::Ctx) -> Option<Value> {
        let theme = opts.theme.as_deref().unwrap_or("none");
        Some(crate::widget::text(format!(
            "guest-rendered entry card [{theme}]: {entry}"
        )))
    }
}

/// An entry renderer that deliberately FAULTS, so the guest half of X15's failure box has something
/// to prove itself against. Upstream's analog is a renderer that `throw`s
/// (`custom-entry.ts:47-52`); a guest panic lowers to a wasm trap, which the host contains as
/// `RenderOutcome::Failed` instead of the silent `None` it used to report.
///
/// `unreachable!` rather than `panic!`: the workspace denies `clippy::panic`, and the trap is
/// identical either way.
struct FaultingEntryRenderer;
impl MessageRenderer for FaultingEntryRenderer {
    fn render_call(
        &self,
        _entry: &Value,
        _opts: &RenderOptions,
        _ctx: &crate::Ctx,
    ) -> Option<Value> {
        unreachable!("demo_boom: this entry renderer always faults (X15 fixture)")
    }
}

pub(super) fn install(api: &mut ExtensionApi) {
    // A custom-MESSAGE renderer (Pi `registerMessageRenderer(customType, renderer)`,
    // `types.ts:1276` @v0.83.0; EXT-036 corrected `:1284`) keyed by a custom message type.
    api.register_message_renderer("demo", DemoRenderer);
    // EXT-006: the per-TOOL renderer for `demo_echo` (whose descriptor declares `has_renderer`).
    // Keyed by the TOOL NAME — that is how the host routes a tool row back to the guest that draws
    // it (Pi `getCallRenderer`/`getResultRenderer`, tool-execution.ts:81-112).
    api.register_message_renderer("demo_echo", DemoToolRenderer);
    // X15 — the custom-ENTRY surface (Pi `registerEntryRenderer(customType, renderer)`,
    // `types.ts:1279` @v0.83.0; EXT-036 corrected `:1295`, the v0.84.1 line for the same symbol — version lag, not fabrication). `demo_card` draws; `demo_boom` deliberately FAULTS, which is the only way to
    // exercise the guest half of the failure box (`custom-entry.ts:47-52`) end to end. A guest
    // panic is a wasm trap, which the host reports as `RenderOutcome::Failed`.
    api.register_entry_renderer("demo_card", DemoEntryRenderer);
    api.register_entry_renderer("demo_boom", FaultingEntryRenderer);
}
