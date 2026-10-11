//! The `/v1/chat/completions` request body.

use super::*;

#[test]
fn build_payload_basic_shape() {
    let m = model_with("codestral-latest", false);
    let opts = StreamOptions {
        max_tokens: Some(1000),
        temperature: Some(0.3),
        ..Default::default()
    };
    let body = build_body(&m, &user_ctx("hello"), &opts);
    assert_eq!(body["model"], "codestral-latest");
    assert_eq!(body["stream"], true);
    assert_eq!(body["max_tokens"], 1000);
    // system prompt prepended.
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["messages"][0]["content"], "be brief");
    assert_eq!(body["messages"][1]["role"], "user");
    assert_eq!(body["messages"][1]["content"][0]["text"], "hello");
    // non-reasoning: no prompt_mode / reasoning_effort.
    assert!(body.get("prompt_mode").is_none());
    assert!(body.get("reasoning_effort").is_none());
}

#[test]
fn prompt_cache_key_set_with_session() {
    let m = model_with("codestral-latest", false);
    let opts = StreamOptions {
        session_id: Some(SessionId::from("s9")),
        ..Default::default()
    };
    let body = build_body(&m, &user_ctx("x"), &opts);
    assert_eq!(body["prompt_cache_key"], "s9");
}

/// PROMPT-001 — the prompt a transcript replays to reaches the chat body.
#[test]
fn the_transcripts_prompt_reaches_the_request_body() {
    use crate::api::prompt_fixture::{agent_context, assert_wire_carries_prompt};
    let m = model_with("codestral-latest", false);
    let body = build_body(&m, &agent_context(Vec::new()), &StreamOptions::default());
    assert_wire_carries_prompt(&body.to_string());
    assert_eq!(body["messages"][0]["role"], "system", "{body}");
}
