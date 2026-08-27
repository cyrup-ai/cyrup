//! Message and tool-choice conversion.

use super::*;

#[test]
fn tool_results_collapse_into_one_user_message() {
    let mut m = model();
    m.reasoning = false;
    let ctx = Context {
        system_prompt: None,
        messages: vec![
            Message::ToolResult {
                tool_call_id: ToolCallId::from("toolu_1"),
                tool_name: "read".to_string(),
                content: vec![Content::text("result A")],
                is_error: false,
                details: None,
                timestamp: 0,
                usage: None,
                added_tool_names: Vec::new(),
            },
            Message::ToolResult {
                tool_call_id: ToolCallId::from("toolu_2"),
                tool_name: "read".to_string(),
                content: vec![Content::text("result B")],
                is_error: false,
                details: None,
                timestamp: 0,
                usage: None,
                added_tool_names: Vec::new(),
            },
        ],
        tools: Vec::new(),
    };
    let body = build_body(&m, &ctx, &StreamOptions::default());
    let msgs = body["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["role"], "user");
    let blocks = msgs[0]["content"].as_array().unwrap();
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0]["type"], "tool_result");
    assert_eq!(blocks[0]["tool_use_id"], "toolu_1");
    assert_eq!(blocks[0]["content"], "result A");
}

#[test]
fn normalize_tool_call_id_rule() {
    assert_eq!(
        normalize_tool_call_id("call/with|bad chars"),
        "call_with_bad_chars"
    );
    let long = "a".repeat(100);
    assert_eq!(normalize_tool_call_id(&long).chars().count(), 64);
}

#[test]
fn tool_choice_mapping() {
    use crate::stream::ToolChoice;
    assert_eq!(tool_choice_wire(&ToolChoice::Auto), json!({"type":"auto"}));
    assert_eq!(
        tool_choice_wire(&ToolChoice::Required),
        json!({"type":"any"})
    );
    assert_eq!(
        tool_choice_wire(&ToolChoice::Function { name: "x".into() }),
        json!({"type":"tool","name":"x"})
    );
}

#[test]
fn redacted_thinking_replays_as_redacted_block() {
    let mut m = model();
    m.reasoning = false;
    let am = AssistantMessage {
        content: vec![Content::Thinking {
            thinking: "[Reasoning redacted]".to_string(),
            thinking_signature: Some("OPAQUE".to_string()),
            redacted: true,
        }],
        provider: ProviderId::from("anthropic"),
        model: "claude-opus-4-5".into(),
        api: API_ID.into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        timestamp: 0,
    };
    let value = build_assistant(&am, false, false).expect("assistant");
    assert_eq!(value["content"][0]["type"], "redacted_thinking");
    assert_eq!(value["content"][0]["data"], "OPAQUE");
}

#[test]
fn empty_signature_thinking_becomes_text_unless_allowed() {
    let am = AssistantMessage {
        content: vec![Content::Thinking {
            thinking: "raw reasoning".to_string(),
            thinking_signature: None,
            redacted: false,
        }],
        provider: ProviderId::from("anthropic"),
        model: "claude-opus-4-5".into(),
        api: API_ID.into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        timestamp: 0,
    };
    // default: convert to text.
    let v = build_assistant(&am, false, false).expect("assistant");
    assert_eq!(v["content"][0]["type"], "text");
    assert_eq!(v["content"][0]["text"], "raw reasoning");
    // allowEmptySignature: keep as thinking with empty signature.
    let v = build_assistant(&am, false, true).expect("assistant");
    assert_eq!(v["content"][0]["type"], "thinking");
    assert_eq!(v["content"][0]["signature"], "");
}
