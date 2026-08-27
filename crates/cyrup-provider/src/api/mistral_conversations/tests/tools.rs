//! Tool encoding and tool-result messages.

use super::*;

#[test]
fn tools_encode_function_shape() {
    let mut ctx = user_ctx("use a tool");
    ctx.tools = vec![ToolDef {
        name: "read".to_string(),
        description: "Read a file".to_string(),
        parameters: json!({ "type": "object", "properties": { "p": { "type": "string" } } }),
        constrained_sampling: None,
    }];
    let m = model_with("codestral-latest", false);
    let opts = StreamOptions {
        tool_choice: Some(ToolChoice::Required),
        ..Default::default()
    };
    let body = build_body(&m, &ctx, &opts);
    assert_eq!(body["tools"][0]["type"], "function");
    assert_eq!(body["tools"][0]["function"]["name"], "read");
    assert_eq!(body["tools"][0]["function"]["strict"], false);
    assert_eq!(body["toolChoice"], "required");
}

#[test]
fn tool_result_message_shape() {
    let messages = vec![Message::ToolResult {
        tool_call_id: CoreToolCallId::from("call12345"),
        tool_name: "read".to_string(),
        content: vec![Content::text("file body")],
        is_error: false,
        details: None,
        timestamp: 0,
        usage: None,
        added_tool_names: Vec::new(),
    }];
    let out = to_chat_messages(&messages, false);
    assert_eq!(out[0]["role"], "tool");
    assert_eq!(out[0]["toolCallId"], "call12345");
    assert_eq!(out[0]["name"], "read");
    assert_eq!(out[0]["content"][0]["text"], "file body");
}
