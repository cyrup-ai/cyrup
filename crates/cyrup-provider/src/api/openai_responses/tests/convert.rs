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
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
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
