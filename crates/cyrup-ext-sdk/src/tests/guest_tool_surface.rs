//! The author-facing half of the guest tool surface: `outputSchema` / `structuredContent`,
//! `annotations` and `prepareLoadout` (pi `ToolDefinition`, `core/extensions/types.ts:592`, `:603`,
//! `:617` @v1.0.4; `AgentToolResult.structuredContent`, `packages/agent/src/types.ts:433`).
//!
//! `guest.rs` only compiles for `wasm32`, so these reach what a host-target test can: the
//! descriptor builders, the loadout the host's JSON becomes, the answer the SDK serializes back,
//! and the dispatch from a tool name to its hook.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::prelude::*;
use serde_json::{Value, json};

struct Hooked;

impl ToolExec for Hooked {
    fn execute(&self, _call: ToolCall) -> Result<ToolOutput, String> {
        Ok(ToolOutput::text("ran"))
    }
    fn prepare_loadout(&self, loadout: &ToolLoadout) -> Option<ToolLoadoutChanges> {
        let mut changes = ToolLoadoutChanges::new().describe(
            "hooked",
            format!("sees {} callable", loadout.callable.len()),
        );
        if loadout.exposure("quiet") == ToolExposure::Hidden {
            changes = changes.hide("quiet");
        }
        Some(changes)
    }
}

#[test]
fn the_descriptor_builders_set_the_schema_the_annotations_and_the_hook_flag() {
    let d = ToolDescriptor::new("t", json!({"type": "object"}));
    assert!(d.output_schema.is_none() && d.annotations.is_none() && !d.prepare_loadout);

    let d = d
        .output_schema(json!({"type": "object"}))
        .annotations(ToolAnnotations::new().read_only(true).destructive(false))
        .prepare_loadout(true);

    assert_eq!(d.output_schema, Some(json!({"type": "object"})));
    assert_eq!(
        d.annotations,
        Some(ToolAnnotations {
            read_only_hint: Some(true),
            destructive_hint: Some(false),
            idempotent_hint: None,
            open_world_hint: None,
        })
    );
    assert!(d.prepare_loadout);
}

#[test]
fn a_tool_output_carries_structured_content_apart_from_its_text() {
    let out = ToolOutput::text("2 files").with_structured_content(json!({"files": 2}));
    assert_eq!(out.structured_content, Some(json!({"files": 2})));
    assert_eq!(ToolOutput::text("x").structured_content, None);
    // An error result keeps it too: pi keeps the structured half of a failed result.
    let failed = ToolOutput::error("boom").with_structured_content(json!({"code": 1}));
    assert!(failed.is_error && failed.structured_content.is_some());
}

/// The JSON the host sends (`events.prepare-loadout`'s `loadout-json`) becomes pi's `ToolLoadout`,
/// and its three lookups read the registered rows.
#[test]
fn the_hosts_loadout_json_answers_pis_three_lookups() {
    let loadout: ToolLoadout = serde_json::from_value(json!({
        "declared": [{"name": "hooked", "exposure": "direct"}],
        "callable": [{"name": "a", "exposure": "codemode"}, {"name": "b", "exposure": "deferred"}],
        "registered": [
            {"name": "hooked", "exposure": "direct", "promptGuidelines": ["use hooked"]},
            {"name": "docs", "exposure": "deferred",
             "namespace": {"name": "mcp__docs", "description": "docs server"}},
            {"name": "quiet", "exposure": "hidden"}
        ]
    }))
    .unwrap();

    assert_eq!(loadout.declared.len(), 1);
    assert_eq!(loadout.callable.len(), 2);
    assert_eq!(loadout.exposure("docs"), ToolExposure::Deferred);
    assert_eq!(loadout.exposure("quiet"), ToolExposure::Hidden);
    assert_eq!(
        loadout.exposure("unknown"),
        ToolExposure::Direct,
        "an unknown name reads as direct, as pi's `?? \"direct\"`"
    );
    assert_eq!(
        loadout.namespace("docs").map(|n| n.name.as_str()),
        Some("mcp__docs")
    );
    assert!(loadout.namespace("hooked").is_none());
    assert_eq!(loadout.prompt_guidelines("hooked"), ["use hooked"]);
    assert!(loadout.prompt_guidelines("unknown").is_empty());
}

#[test]
fn changes_serialize_as_pis_shape_and_omit_what_is_empty() {
    assert_eq!(
        serde_json::to_value(ToolLoadoutChanges::new()).unwrap(),
        json!({})
    );
    let changes = ToolLoadoutChanges::new().describe("a", "new").hide("b");
    assert_eq!(
        serde_json::to_value(changes).unwrap(),
        json!({"descriptions": {"a": "new"}, "hiddenDeclarations": ["b"]})
    );
}

#[test]
fn the_api_runs_the_hook_of_the_named_tool_and_only_that_tool() {
    let mut api = ExtensionApi::new();
    api.register_tool(
        ToolDescriptor::new("hooked", json!({"type": "object"})).prepare_loadout(true),
        Hooked,
    );
    api.register_tool(
        ToolDescriptor::new("plain", json!({"type": "object"})),
        |_call: ToolCall| Ok(ToolOutput::text("x")),
    );
    let loadout: ToolLoadout = serde_json::from_value(json!({
        "callable": [{"name": "a", "exposure": "codemode"}],
        "registered": [{"name": "quiet", "exposure": "hidden"}]
    }))
    .unwrap();

    let changes = api.prepare_tool_loadout("hooked", &loadout).unwrap();
    let wire: Value = serde_json::to_value(changes).unwrap();
    assert_eq!(
        wire,
        json!({"descriptions": {"hooked": "sees 1 callable"}, "hiddenDeclarations": ["quiet"]}),
        "the hook read the loadout it was handed"
    );
    assert!(api.prepare_tool_loadout("plain", &loadout).is_none());
    assert!(api.prepare_tool_loadout("missing", &loadout).is_none());
}
