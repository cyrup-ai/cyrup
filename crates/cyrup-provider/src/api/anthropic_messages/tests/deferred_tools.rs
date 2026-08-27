//! DRIFT-001: message-anchored tool loading (Pi `deferred-tools.test.ts`)
//!
//! These assert the EMITTED WIRE JSON, not helper return values. The rule that makes them
//! load-bearing: Anthropic REJECTS a `tool_result` whose content mixes `tool_reference` with
//! ordinary blocks, so the real content must be DISPLACED into siblings — relocated, never
//! dropped.

use super::*;

#[test]
fn deferred_tool_is_marked_defer_loading_and_anchored_by_a_tool_reference() {
    // Pi "loads an Anthropic tool at its tool-result marker".
    let ctx = deferred_ctx(
        vec![tool_def("base_tool"), tool_def("late_tool")],
        &["late_tool"],
    );
    let opts = StreamOptions {
        cache_retention: Some(CacheRetention::None),
        ..Default::default()
    };
    let body = build_body(&opus_4_6(), &ctx, &opts);

    // EXACT tools array. `defer_loading` sits AFTER `input_schema` (Pi key order) and the
    // immediate tool carries no marker at all.
    assert_eq!(
        body["tools"],
        json!([
            {
                "name": "base_tool",
                "description": "The base_tool tool",
                "eager_input_streaming": true,
                "input_schema": { "type": "object", "properties": {}, "required": [] }
            },
            {
                "name": "late_tool",
                "description": "The late_tool tool",
                "eager_input_streaming": true,
                "input_schema": { "type": "object", "properties": {}, "required": [] },
                "defer_loading": true
            }
        ])
    );

    // EXACT tool-result user message: the reference REPLACES the content, and the displaced
    // text follows the tool_result as a sibling.
    assert_eq!(
        tool_result_content(&body),
        vec![
            json!({
                "type": "tool_result",
                "tool_use_id": "call_1",
                "content": [{ "type": "tool_reference", "tool_name": "late_tool" }],
                "is_error": false
            }),
            json!({ "type": "text", "text": "done" }),
        ]
    );
    // Constraint 5: the original content still EXISTS in the payload.
    assert!(
        serde_json::to_string(&body)
            .expect("json")
            .contains("\"done\""),
        "displaced tool output must be relocated, never dropped"
    );
}

#[test]
fn a_tool_reference_is_never_mixed_with_ordinary_content_in_one_tool_result() {
    // Constraint 4, stated directly as an invariant over the whole payload: any tool_result
    // whose content is an array is EITHER all tool_reference OR all ordinary blocks.
    let mut ctx = deferred_ctx(
        vec![tool_def("base_tool"), tool_def("late_tool")],
        &["late_tool"],
    );
    if let Some(Message::ToolResult { content, .. }) = ctx.messages.get_mut(2) {
        *content = vec![
            Content::text("work completed"),
            Content::Image {
                data: "aW1hZ2U=".to_string(),
                mime_type: "image/png".to_string(),
            },
        ];
    }
    let body = build_body(&opus_4_6(), &ctx, &StreamOptions::default());

    let mut saw_reference = false;
    for m in body["messages"].as_array().expect("messages") {
        let Some(blocks) = m["content"].as_array() else {
            continue;
        };
        for b in blocks {
            if b["type"] != "tool_result" {
                continue;
            }
            let Some(inner) = b["content"].as_array() else {
                continue;
            };
            let refs = inner
                .iter()
                .filter(|x| x["type"] == "tool_reference")
                .count();
            if refs > 0 {
                saw_reference = true;
                assert_eq!(
                    refs,
                    inner.len(),
                    "tool_result mixes tool_reference with ordinary blocks — Anthropic 400s: {b:#}"
                );
            }
        }
    }
    assert!(saw_reference, "expected at least one tool_reference");
}

#[test]
fn displaced_content_is_flushed_after_every_tool_result_of_the_batch() {
    // Pi "preserves tool output as sibling content after emitting references". The
    // displacement of the FIRST result lands AFTER the SECOND result's block — siblings are
    // accumulated across the whole consecutive run and flushed once. Per-block interleaving
    // fails this.
    let mut ctx = deferred_ctx(
        vec![tool_def("base_tool"), tool_def("late_tool")],
        &["late_tool"],
    );
    ctx.messages[1] = tc_assistant(&[("call_1", "base_tool"), ("call_2", "base_tool")]);
    if let Some(Message::ToolResult { content, .. }) = ctx.messages.get_mut(2) {
        *content = vec![
            Content::text("work completed"),
            Content::Image {
                data: "aW1hZ2U=".to_string(),
                mime_type: "image/png".to_string(),
            },
        ];
    }
    ctx.messages
        .insert(3, tr("call_2", vec![Content::text("second result")], &[]));

    let opts = StreamOptions {
        cache_retention: Some(CacheRetention::None),
        ..Default::default()
    };
    let body = build_body(&opus_4_6(), &ctx, &opts);

    assert_eq!(
        tool_result_content(&body),
        vec![
            json!({
                "type": "tool_result",
                "tool_use_id": "call_1",
                "content": [{ "type": "tool_reference", "tool_name": "late_tool" }],
                "is_error": false
            }),
            json!({
                "type": "tool_result",
                "tool_use_id": "call_2",
                "content": "second result",
                "is_error": false
            }),
            json!({ "type": "text", "text": "work completed" }),
            json!({
                "type": "image",
                "source": { "type": "base64", "media_type": "image/png", "data": "aW1hZ2U=" }
            }),
        ]
    );
}

#[test]
fn a_deferred_name_is_referenced_at_most_once_per_request() {
    // `loadedToolNames` is declared once per convertMessages call (Pi :1125), so a second
    // marker for the same tool emits no reference and displaces nothing.
    let mut ctx = deferred_ctx(
        vec![tool_def("base_tool"), tool_def("late_tool")],
        &["late_tool"],
    );
    ctx.messages
        .insert(3, tc_assistant(&[("call_2", "base_tool")]));
    ctx.messages.insert(
        4,
        tr("call_2", vec![Content::text("again")], &["late_tool"]),
    );

    let body = build_body(&opus_4_6(), &ctx, &StreamOptions::default());
    let refs = serde_json::to_string(&body)
        .expect("json")
        .matches("\"tool_reference\"")
        .count();
    assert_eq!(refs, 1, "a deferred name must be referenced exactly once");
    // The second result keeps its own content inline (no references → no displacement).
    let second = body["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find_map(|m| {
            m["content"]
                .as_array()?
                .iter()
                .find(|b| b["tool_use_id"] == "call_2")
                .cloned()
        })
        .expect("second tool_result");
    assert_eq!(second["content"], json!("again"));
}

#[test]
fn a_tool_used_before_its_marker_stays_immediate() {
    // Pi "keeps a tool immediate when it was used before its marker".
    let mut ctx = deferred_ctx(
        vec![tool_def("base_tool"), tool_def("late_tool")],
        &["late_tool"],
    );
    ctx.messages[1] = tc_assistant(&[("call_1", "late_tool")]);
    let body = build_body(&opus_4_6(), &ctx, &StreamOptions::default());

    assert_eq!(tool_names(&body), ["base_tool", "late_tool"]);
    assert!(
        body["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .all(|t| t.get("defer_loading").is_none())
    );
    assert!(
        !serde_json::to_string(&body)
            .expect("json")
            .contains("tool_reference")
    );
}

#[test]
fn a_marked_tool_absent_from_the_active_set_is_not_resurrected() {
    // Pi "does not resurrect a marked tool missing from Context.tools".
    let ctx = deferred_ctx(vec![tool_def("base_tool")], &["late_tool"]);
    let body = build_body(&opus_4_6(), &ctx, &StreamOptions::default());
    assert_eq!(tool_names(&body), ["base_tool"]);
    assert!(
        !serde_json::to_string(&body)
            .expect("json")
            .contains("tool_reference")
    );
}

#[test]
fn the_safety_valve_promotes_every_tool_back_when_all_are_deferred() {
    // Pi "keeps one immediate Anthropic tool when every current tool is marked"
    // (anthropic-messages.ts:955-959). Anthropic rejects a request whose every tool is
    // deferred, so the valve fires HERE — and only here; openai-responses has none.
    let ctx = deferred_ctx(vec![tool_def("late_tool")], &["late_tool"]);
    let body = build_body(&opus_4_6(), &ctx, &StreamOptions::default());

    assert_eq!(tool_names(&body), ["late_tool"]);
    assert!(body["tools"][0].get("defer_loading").is_none());
    assert!(
        !serde_json::to_string(&body)
            .expect("json")
            .contains("tool_reference")
    );
}

#[test]
fn cache_control_marks_the_last_immediate_tool_never_a_deferred_one() {
    // Pi passes `undefined` cacheControl to the deferred convertTools call (:1015-1021), so
    // the cache breakpoint stays inside the stable prefix.
    let ctx = deferred_ctx(
        vec![tool_def("base_tool"), tool_def("late_tool")],
        &["late_tool"],
    );
    let body = build_body(&opus_4_6(), &ctx, &StreamOptions::default());
    assert_eq!(
        body["tools"][0]["cache_control"],
        json!({ "type": "ephemeral" })
    );
    assert!(body["tools"][1].get("cache_control").is_none());
    assert_eq!(body["tools"][1]["defer_loading"], json!(true));
}

#[test]
fn cache_control_lands_on_the_displaced_sibling_not_the_reference_block() {
    // The last block of a reference-bearing user message is now a displaced `text`, and
    // `applyLastUserCacheControl` marks it there (Pi :1259-1268). Only true when the
    // tool-result batch is the LAST message.
    let mut ctx = deferred_ctx(
        vec![tool_def("base_tool"), tool_def("late_tool")],
        &["late_tool"],
    );
    ctx.messages.pop(); // drop the trailing user turn
    let body = build_body(&opus_4_6(), &ctx, &StreamOptions::default());
    let content = tool_result_content(&body);
    assert_eq!(
        content.last().expect("last block"),
        &json!({ "type": "text", "text": "done", "cache_control": { "type": "ephemeral" } })
    );
    assert!(content[0].get("cache_control").is_none());
}
