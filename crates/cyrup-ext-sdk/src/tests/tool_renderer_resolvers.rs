//! EXT-107 — the guest half of pi `pi.registerToolRenderer`: a declaration routes a tool name the
//! guest did not register to the declared renderer, and an exact renderer keeps its own key.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use crate::prelude::*;
use serde_json::json;

struct Marker(&'static str);
impl MessageRenderer for Marker {
    fn render_call(
        &self,
        _call: &serde_json::Value,
        _opts: &crate::RenderOptions,
        _c: &Ctx,
    ) -> Option<serde_json::Value> {
        Some(json!(format!("call:{}", self.0)))
    }
    fn render_result(
        &self,
        _result: &serde_json::Value,
        _opts: &crate::RenderOptions,
        _c: &Ctx,
    ) -> Option<serde_json::Value> {
        Some(json!(format!("result:{}", self.0)))
    }
}

/// A key only a pattern matches reaches the pattern's renderer on both surfaces; a key nothing
/// matches still answers `None`.
#[test]
fn a_pattern_routes_an_unregistered_tool_name_to_its_renderer() {
    let mut api = ExtensionApi::new();
    api.register_tool_renderer("mcp__*", ToolRendererMode::FillIn, Marker("family"));
    let opts = crate::RenderOptions::default();
    assert_eq!(
        api.render_call("mcp__srv__tool", &json!({}), &opts),
        Some(json!("call:family"))
    );
    assert_eq!(
        api.render_result("mcp__srv__tool", &json!({}), &opts),
        Some(json!("result:family"))
    );
    assert!(api.render_call("bash", &json!({}), &opts).is_none());
}

/// An exact renderer for a key the pattern also matches keeps drawing it, and among this guest's
/// own declarations the first one declared draws.
#[test]
fn an_exact_renderer_and_the_first_declaration_win_inside_one_guest() {
    let mut api = ExtensionApi::new();
    api.register_tool_renderer("demo_*", ToolRendererMode::Claim, Marker("first"));
    api.register_tool_renderer("demo_x*", ToolRendererMode::Claim, Marker("second"));
    api.register_message_renderer("demo_exact", Marker("exact"));
    let opts = crate::RenderOptions::default();
    assert_eq!(
        api.render_call("demo_exact", &json!({}), &opts),
        Some(json!("call:exact"))
    );
    assert_eq!(
        api.render_call("demo_xy", &json!({}), &opts),
        Some(json!("call:first"))
    );
}

/// The SDK's matcher is the host's `*` rule.
#[test]
fn the_pattern_rule_is_star_globbing() {
    use crate::api::tool_pattern_matches as m;
    assert!(m("mcp__*__*", "mcp__s__t"));
    assert!(!m("mcp__*__*", "mcp__s_t"));
    assert!(m("*_ghost", "a_ghost"));
    assert!(!m("*_ghost", "a_ghosts"));
    assert!(m("exact", "exact"));
    assert!(!m("exact", "exactly"));
    assert!(!m("ab*ba", "aba"));
}
