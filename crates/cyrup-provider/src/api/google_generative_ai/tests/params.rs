//! The `:streamGenerateContent` request body.

use super::*;

#[test]
fn build_params_basic_shape() {
    let m = model_with("gemini-2.0-flash", false);
    let opts = StreamOptions {
        max_tokens: Some(1000),
        temperature: Some(0.4),
        ..Default::default()
    };
    let body = build_body(&m, &user_ctx("hello"), &opts);
    assert_eq!(body["contents"][0]["role"], "user");
    assert_eq!(body["contents"][0]["parts"][0]["text"], "hello");
    assert_eq!(body["systemInstruction"]["parts"][0]["text"], "be brief");
    assert_eq!(body["generationConfig"]["maxOutputTokens"], 1000);
    assert!((body["generationConfig"]["temperature"].as_f64().unwrap() - 0.4).abs() < 1e-6);
    // Non-reasoning model: no thinkingConfig.
    assert!(body["generationConfig"].get("thinkingConfig").is_none());
}

/// PROMPT-001 — the prompt a transcript replays to reaches the `systemInstruction` of the body.
#[test]
fn the_transcripts_prompt_reaches_the_request_body() {
    use crate::api::prompt_fixture::{agent_context, assert_wire_carries_prompt};
    let m = model_with("gemini-2.0-flash", false);
    let body = build_body(&m, &agent_context(Vec::new()), &StreamOptions::default());
    assert_wire_carries_prompt(&body["systemInstruction"].to_string());
}
