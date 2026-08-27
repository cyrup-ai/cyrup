//! The `Content[]` encoder.

use super::*;

#[test]
fn function_response_uses_output_and_error_keys() {
    let m = model_with("gemini-2.5-pro", true);
    let ctx = Context {
        system_prompt: None,
        messages: vec![
            Message::User {
                content: vec![Content::text("hi")],
                timestamp: 0,
            },
            Message::ToolResult {
                tool_call_id: cyrup_core::ToolCallId::from("c1"),
                tool_name: "read".to_string(),
                content: vec![Content::text("file body")],
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
    let contents = body["contents"].as_array().unwrap();
    // The tool result becomes a `user` turn with a functionResponse part.
    let fr = contents
        .iter()
        .find_map(|c| c["parts"][0].get("functionResponse"))
        .expect("functionResponse part");
    assert_eq!(fr["name"], "read");
    assert_eq!(fr["response"]["output"], "file body");
    // gemini-2.5-pro is < 3, so no `id` field (requiresToolCallId false for gemini).
    assert!(fr.get("id").is_none());
}

/// VERSION LAG (v0.83.0 → v0.84.1): `requiresToolCallId` gained a third arm
/// `geminiMajorVersion !== undefined && geminiMajorVersion >= 3`
/// (v0.84.1 `ai/src/api/google-shared.ts:72-79`), so Gemini 3 echoes explicit tool-call ids in
/// both `functionResponse` (`:215` — `...(includeId ? { id: msg.toolCallId } : {})`) and
/// `functionCall` (`:176`). At v0.83.0 (`google-shared.ts:71-73`) only `claude-`/`gpt-oss-`
/// qualified, so every Gemini id took the `false` branch.
#[test]
fn gemini3_echoes_tool_call_ids() {
    assert!(requires_tool_call_id("gemini-3-pro-preview"));
    assert!(requires_tool_call_id("gemini-live-3-flash"));
    // MIRROR: the two pre-existing arms and every sub-3 Gemini are unchanged.
    assert!(requires_tool_call_id("claude-opus-4-5"));
    assert!(requires_tool_call_id("gpt-oss-120b"));
    assert!(!requires_tool_call_id("gemini-2.5-pro"));
    assert!(!requires_tool_call_id("gemini-1.5-flash"));
    assert!(!requires_tool_call_id("gemma-4-2b"));

    // End-to-end: the `id` reaches the wire body for a Gemini 3 model.
    let m = model_with("gemini-3-pro-preview", true);
    let ctx = Context {
        system_prompt: None,
        messages: vec![Message::ToolResult {
            tool_call_id: cyrup_core::ToolCallId::from("call_1"),
            tool_name: "read".to_string(),
            content: vec![Content::text("file body")],
            is_error: false,
            details: None,
            timestamp: 0,
            usage: None,
            added_tool_names: Vec::new(),
        }],
        tools: Vec::new(),
    };
    let body = build_body(&m, &ctx, &StreamOptions::default());
    let contents = body["contents"].as_array().unwrap();
    let fr = contents
        .iter()
        .find_map(|c| c["parts"][0].get("functionResponse"))
        .expect("functionResponse part");
    assert_eq!(fr["id"], "call_1");
}

/// DRIFT-048. The `functionCall.id` presence rule is `requiresToolCallId(model.id)` —
/// the model this request is being SENT to (`google-shared.ts:177`) — not the model that
/// produced the historical assistant turn. cyrup re-derived it from `am.model` inside
/// `assistant_parts`, so on a mid-session switch the `functionCall` and its matching
/// `functionResponse` disagreed: `convert_messages` already keyed the response on the target.
///
/// RED before the fix on BOTH directions; the same-model cases below are unaffected either way
/// and are asserted so the fix cannot be a blanket flip.
#[test]
fn drift048_tool_call_id_follows_the_target_model_not_the_source_message() {
    fn convert_for(target: &str, source_model: &str) -> (Vec<Value>, Vec<Value>) {
        let model = model_with(target, true);
        let mut ctx = signed_block_ctx("google", source_model, vec![a_tool_call()]);
        ctx.messages.push(Message::ToolResult {
            tool_name: "bash".to_string(),
            tool_call_id: ToolCallId::from("call_1"),
            content: vec![Content::text("ok")],
            is_error: false,
            details: None,
            usage: None,
            added_tool_names: Vec::new(),
            timestamp: 2,
        });
        let contents = convert_messages(&model, &ctx);
        let calls = model_turn_parts(&contents);
        let responses = contents
            .iter()
            .filter(|c| c["role"] == "user")
            .filter_map(|c| c["parts"].as_array().cloned())
            .flatten()
            .filter(|p| p.get("functionResponse").is_some())
            .collect::<Vec<_>>();
        (calls, responses)
    }

    // Old turn on 2.5, request targeted at 3 ⇒ BOTH halves carry the id.
    let (calls, responses) = convert_for("gemini-3-pro-preview", "gemini-2.5-pro");
    assert_eq!(
        calls[0]["functionCall"]["id"], "call_1",
        "the target model requires ids, so the historical call must carry one too"
    );
    assert_eq!(responses[0]["functionResponse"]["id"], "call_1");

    // Reverse switch: old turn on 3, request targeted at 2.5 ⇒ NEITHER half carries an id.
    let (calls, responses) = convert_for("gemini-2.5-pro", "gemini-3-pro-preview");
    assert!(
        calls[0]["functionCall"].get("id").is_none(),
        "an id upstream would never send: {}",
        calls[0]
    );
    assert!(responses[0]["functionResponse"].get("id").is_none());

    // Same-model cases are unchanged in both directions.
    let (calls, responses) = convert_for("gemini-3-pro-preview", "gemini-3-pro-preview");
    assert_eq!(calls[0]["functionCall"]["id"], "call_1");
    assert_eq!(responses[0]["functionResponse"]["id"], "call_1");

    let (calls, responses) = convert_for("gemini-2.5-pro", "gemini-2.5-pro");
    assert!(calls[0]["functionCall"].get("id").is_none());
    assert!(responses[0]["functionResponse"].get("id").is_none());
}
