//! `ToolCall::execute_tool` / `ToolCall::tools` — pi `ctx.executeTool` / `ctx.tools`
//! (`core/extensions/types.ts:367-395` @v1.0.1) — as far as the host target can reach them: the
//! shapes the host's answer is read into, and the refusal the host-target arm gives.
//!
//! The imports themselves are driven end to end by `cyrup-it`'s `wasm_nested_tool_calls`; what is
//! pinned here is that the SDK reads pi's `AgentToolCallOutcome` and the callable-tool rows the
//! host emits.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::{CallableTool, ExecuteToolOptions, NestedCallError, ToolCall, ToolOutcome};
use serde_json::json;

/// pi `AgentToolCallOutcome` — `{toolCall, result, isError}` — with the optional `result` members
/// the host emits only when the tool set them.
#[test]
fn the_outcome_reads_pi_agent_tool_call_outcome() {
    let outcome: ToolOutcome = serde_json::from_value(json!({
        "toolCall": { "type": "toolCall", "id": "c/1", "name": "read", "arguments": {} },
        "result": {
            "content": [
                { "type": "text", "text": "line one" },
                { "type": "image", "data": "...", "mimeType": "image/png" },
                { "type": "text", "text": "line two" }
            ],
            "details": { "truncated": false },
            "structuredContent": { "n": 1 },
            "isError": true,
            "terminate": true,
            "addedToolNames": ["extra"]
        },
        "isError": false
    }))
    .unwrap();

    assert_eq!(outcome.tool_call["id"], "c/1");
    assert!(
        !outcome.is_error,
        "the verdict is the outcome's, not the tool's"
    );
    assert!(outcome.result.is_error, "the tool's own flag is kept apart");
    assert_eq!(outcome.result.text(), "line one\nline two");
    assert_eq!(outcome.result.structured_content, Some(json!({ "n": 1 })));
    assert_eq!(outcome.result.details, Some(json!({ "truncated": false })));
    assert_eq!(outcome.result.terminate, Some(true));
    assert_eq!(outcome.result.added_tool_names, ["extra"]);
}

/// The host omits every `result` member the tool did not set.
#[test]
fn a_bare_outcome_reads_with_every_optional_member_absent() {
    let outcome: ToolOutcome = serde_json::from_value(json!({
        "toolCall": { "type": "toolCall", "id": "c/1", "name": "read", "arguments": {} },
        "result": { "content": [{ "type": "text", "text": "ok" }] },
        "isError": true
    }))
    .unwrap();
    assert!(outcome.is_error);
    assert!(!outcome.result.is_error);
    assert_eq!(outcome.result.details, None);
    assert_eq!(outcome.result.structured_content, None);
    assert_eq!(outcome.result.terminate, None);
    assert!(outcome.result.added_tool_names.is_empty());
}

/// The rows of `host-tool.callable-tools`.
#[test]
fn callable_tools_read_what_the_host_emits() {
    let tools: Vec<CallableTool> = serde_json::from_value(json!([
        {
            "name": "read",
            "label": "Read",
            "description": "Read a file",
            "parameters": { "type": "object" },
            "exposure": "direct"
        },
        {
            "name": "mcp__docs__search",
            "description": "Search",
            "parameters": {},
            "outputSchema": { "type": "object" },
            "exposure": "codemode",
            "namespace": { "name": "mcp__docs", "description": "Docs server" }
        }
    ]))
    .unwrap();
    assert_eq!(tools[0].name, "read");
    assert_eq!(tools[0].label.as_deref(), Some("Read"));
    assert_eq!(tools[0].exposure, "direct");
    assert!(tools[0].namespace.is_none());
    assert_eq!(tools[1].exposure, "codemode");
    assert_eq!(tools[1].output_schema, Some(json!({ "type": "object" })));
    let ns = tools[1].namespace.as_ref().unwrap();
    assert_eq!(ns.name, "mcp__docs");
    assert_eq!(ns.description.as_deref(), Some("Docs server"));
    assert_eq!(ns.instructions, None);
}

/// With no host there is nothing to call: the host-target arm refuses by name instead of
/// fabricating an outcome (the arm rule in `ctx/mod.rs`).
#[test]
fn the_host_target_refuses_instead_of_fabricating_an_outcome() {
    let call = ToolCall::new("c", json!({}));
    let err = call
        .execute_tool("read", json!({}), ExecuteToolOptions::default())
        .unwrap_err();
    assert_eq!(
        err,
        NestedCallError::Host("execute_tool unavailable on host target".into())
    );
    assert_eq!(
        call.tools().unwrap_err(),
        NestedCallError::Host("tools unavailable on host target".into())
    );
}

/// Arguments that cannot be encoded are the author's error, reported before any host call.
#[test]
fn arguments_that_are_not_json_are_refused_before_the_call() {
    // A map with a non-string key has no JSON form.
    let mut bad = std::collections::BTreeMap::new();
    bad.insert((1, 2), "v");
    let err = ToolCall::new("c", json!({}))
        .execute_tool("read", bad, ExecuteToolOptions::default())
        .unwrap_err();
    assert!(matches!(err, NestedCallError::Arguments(_)), "{err:?}");
}
