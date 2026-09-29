//! Message transformation: tool-call id normalisation on replay.

use super::*;

/// DRIFT-002: Responses-API ids are `{call_id}|{item_id}`. Two calls in the same turn can share
/// a `call_id`; keeping only that half collapses them into one id, and Chat Completions rejects
/// the replayed request with a 400 for duplicate `tool_call_id`s (and the tool results then
/// point at the wrong call). Pi keeps both halves.
#[test]
fn shared_call_id_with_distinct_item_ids_stays_distinct_on_replay() {
    let ctx = ctx_with_tool_call_ids(&["call_abc|item_1", "call_abc|item_2"]);
    let body = build_body(&model(), &ctx, &StreamOptions::default());
    let messages = body["messages"].as_array().unwrap();

    let tcs = messages[0]["tool_calls"].as_array().unwrap();
    assert_eq!(tcs.len(), 2);
    let first = tcs[0]["id"].as_str().unwrap();
    let second = tcs[1]["id"].as_str().unwrap();
    assert_ne!(
        first, second,
        "both tool calls replayed as `{first}` — a duplicate tool_call_id is a provider 400"
    );
    assert_eq!(first, "call_abc_item_1");
    assert_eq!(second, "call_abc_item_2");

    // The tool results must follow their own call, not both bind to the first.
    assert_eq!(messages[1]["tool_call_id"], "call_abc_item_1");
    assert_eq!(messages[2]["tool_call_id"], "call_abc_item_2");
}

/// The over-40-char fallback: `{call-prefix}_{8-char shortHash of the WHOLE id}`, so two ids
/// sharing a call_id still differ.
#[test]
fn overlong_pipe_ids_fall_back_to_a_hash_suffix() {
    let long_item = "a".repeat(400);
    let id_a = format!("call_abc|{long_item}1");
    let id_b = format!("call_abc|{long_item}2");
    let ctx = ctx_with_tool_call_ids(&[id_a.as_str(), id_b.as_str()]);
    let body = build_body(&model(), &ctx, &StreamOptions::default());
    let tcs = body["messages"][0]["tool_calls"].as_array().unwrap();
    let a = tcs[0]["id"].as_str().unwrap();
    let b = tcs[1]["id"].as_str().unwrap();
    assert_ne!(a, b, "hashed ids collided: {a}");
    assert!(a.len() <= 40, "id too long for OpenAI: {a} ({})", a.len());
    assert!(b.len() <= 40, "id too long for OpenAI: {b} ({})", b.len());
    assert_eq!(a, format!("call_abc_{}", &short_hash(&id_a)[..8]));
    assert_eq!(b, format!("call_abc_{}", &short_hash(&id_b)[..8]));
}

/// Unit coverage for the branches the wire test cannot reach cheaply.
#[test]
fn normalize_tool_call_id_matches_pi() {
    let m = model();
    // No pipe, non-openai provider: untouched.
    assert_eq!(normalize_tool_call_id(&m, "call_plain"), "call_plain");
    // Empty item id keeps just the sanitized call id (Pi's `itemId.length > 0` guard).
    assert_eq!(normalize_tool_call_id(&m, "call_abc|"), "call_abc");
    // Disallowed characters in either half become `_`.
    assert_eq!(
        normalize_tool_call_id(&m, "call+a/b=|item=1"),
        "call_a_b__item_1"
    );
    // Exactly 40 chars is kept whole (`<= 40`).
    let exact = format!("call_{}|{}", "a".repeat(17), "b".repeat(17));
    assert_eq!(normalize_tool_call_id(&m, &exact).len(), 40);
    // A single-char call id still keeps at least one char of prefix (`Math.max(1, …)`).
    let tiny = format!("c|{}", "d".repeat(60));
    let out = normalize_tool_call_id(&m, &tiny);
    assert_eq!(out, format!("c_{}", &short_hash(&tiny)[..8]));
}

/// PROV-083a — pi `transform-messages.ts:164-166`/`:216-221` @v0.87.1, verbatim: *"System messages
/// are transparent to tool-call accounting: one that lands between a tool call and its results is
/// held back and emitted after the results (synthetic ones included), so it never causes a duplicate
/// result for a call that is answered later."*
///
/// Red before the change in two ways: the arm did not exist (a compile error), and once it existed
/// as a plain `result.push(msg)` the system message split the assistant/toolResult run, so the
/// synthetic "No result provided" result landed AFTER it instead of before.
#[test]
fn a_system_message_between_a_tool_call_and_its_results_is_held_back() {
    let sys = |ts: i64| {
        Message::System(cyrup_core::SystemMessage {
            content: vec![Content::text("mid")],
            sections: None,
            tools_added: Vec::new(),
            tools_removed: Vec::new(),
            timestamp: ts,
        })
    };
    let roles = |msgs: &[Message]| -> Vec<&'static str> {
        msgs.iter()
            .map(|m| match m {
                Message::System(_) => "system",
                Message::User { .. } => "user",
                Message::Assistant(_) => "assistant",
                Message::ToolResult { .. } => "toolResult",
            })
            .collect()
    };

    // One assistant tool call, then a system message, then NOTHING: the synthetic result for the
    // orphaned call is emitted FIRST, and the held system message follows it.
    let ctx = ctx_with_tool_call_ids(&["call_1"]);
    let assistant = ctx.messages[0].clone();
    let out = transform_messages(&[assistant.clone(), sys(1)], &model());
    assert_eq!(roles(&out), ["assistant", "toolResult", "system"]);
    let Some(Message::ToolResult { is_error, .. }) = out.get(1) else {
        panic!("expected the synthesized result, got {out:?}");
    };
    assert!(*is_error, "the synthesized orphan result is an error");

    // The real result answers the call, so the system message still lands after it.
    let real_result = ctx.messages[1].clone();
    let out = transform_messages(&[assistant.clone(), sys(1), real_result.clone()], &model());
    assert_eq!(roles(&out), ["assistant", "toolResult", "system"]);

    // With NO pending call, a system message is emitted in place (`:218-220`).
    let out = transform_messages(&[sys(0), assistant, real_result], &model());
    assert_eq!(roles(&out), ["system", "assistant", "toolResult"]);

    // And a system message on its own passes straight through the first pass untouched (`:79-82`).
    assert_eq!(transform_messages(&[sys(5)], &model()), vec![sys(5)]);
}
