//! Message conversion and id normalization.

use super::*;

#[test]
fn assistant_text_replay_carries_message_item() {
    let mut ctx = user_ctx("hi");
    let am = AssistantMessage {
        content: vec![Content::text("prior answer")],
        provider: "openai".into(),
        model: "gpt-5".into(),
        api: API_ID.into(),
        response_model: None,
        response_id: None,
        provider_thinking_level: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 0,
    };
    ctx.messages.push(Message::Assistant(am));
    let body = build_params(&model(), &ctx, &StreamOptions::default(), None);
    let input = body["input"].as_array().unwrap();
    // user + assistant message item.
    let assistant = input.iter().find(|m| m["type"] == "message").unwrap();
    assert_eq!(assistant["role"], "assistant");
    assert_eq!(assistant["content"][0]["type"], "output_text");
    assert_eq!(assistant["content"][0]["text"], "prior answer");
    // fallback id for a signature-less first text block.
    assert!(assistant["id"].as_str().unwrap().starts_with("msg_pi_"));
}

#[test]
fn normalize_id_part_sanitizes_and_trims() {
    assert_eq!(normalize_id_part("abc|def#ghi"), "abc_def_ghi");
    assert_eq!(normalize_id_part("trailing___"), "trailing");
    assert_eq!(normalize_id_part(&"x".repeat(100)).chars().count(), 64);
}

/// An assistant turn from `openai`/`gpt-5` over the Responses api (the [`model`] fixture) that made
/// one namespaced `function_call` (DRIFT-058).
fn namespaced_call_context() -> Context {
    let mut ctx = user_ctx("hi");
    let mut args = serde_json::Map::new();
    args.insert("value".to_string(), json!("hello"));
    ctx.messages.push(Message::Assistant(AssistantMessage {
        content: vec![Content::ToolCall(ToolCall {
            id: ToolCallId::from("call_test|fc_test"),
            name: "lookup".to_string(),
            arguments: args.into(),
            thought_signature: None,
            namespace: Some("dynamic_tools".to_string()),
        })],
        provider: "openai".into(),
        model: "gpt-5".into(),
        api: API_ID.into(),
        response_model: None,
        response_id: None,
        provider_thinking_level: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::ToolUse,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 0,
    }));
    ctx
}

fn replayed_function_call(target: &Model) -> Value {
    let body = build_params(
        target,
        &namespaced_call_context(),
        &StreamOptions::default(),
        None,
    );
    body["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["type"] == "function_call")
        .cloned()
        .expect("a replayed function_call item")
}

#[test]
fn same_model_replay_carries_the_namespace() {
    // Pi `openai-responses-namespace.test.ts` "round-trips a function namespace": the replayed
    // `function_call` keeps `namespace` when provider, api AND model all match.
    let item = replayed_function_call(&model());
    assert_eq!(item["namespace"], "dynamic_tools", "{item}");
    assert_eq!(item["id"], "fc_test");
    assert_eq!(item["call_id"], "call_test");
    assert_eq!(item["name"], "lookup");
    assert_eq!(item["arguments"], r#"{"value":"hello"}"#);
}

#[test]
fn a_different_model_provider_or_api_replays_without_the_namespace() {
    // Pi "drops namespaces when the target cannot replay their load items": `isSameModel` needs
    // the provider, the api and the model id to match. The first case is also `isDifferentModel`;
    // the other two are not, which is why the namespace gate is its own flag.
    let mut other_model = model();
    other_model.id = "gpt-5.2".into();
    let mut other_provider = model();
    other_provider.provider = "azure-openai-responses".into();
    let mut other_api = model();
    other_api.api = "openai-codex-responses".into();
    other_api.provider = "openai-codex".into();
    for (label, target) in [
        ("different model", other_model),
        ("different provider", other_provider),
        ("different api", other_api),
    ] {
        let item = replayed_function_call(&target);
        assert!(
            item.get("namespace").is_none(),
            "{label} must not replay the namespace: {item}"
        );
        assert_eq!(item["name"], "lookup", "{label}: the call itself is kept");
    }
}

#[test]
fn an_ordinary_call_replays_without_a_namespace_key() {
    let mut ctx = namespaced_call_context();
    if let Some(Message::Assistant(am)) = ctx.messages.last_mut() {
        for c in &mut am.content {
            if let Content::ToolCall(tc) = c {
                tc.namespace = None;
            }
        }
    }
    let body = build_params(&model(), &ctx, &StreamOptions::default(), None);
    let item = body["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["type"] == "function_call")
        .unwrap();
    assert!(item.get("namespace").is_none(), "{item}");
}
