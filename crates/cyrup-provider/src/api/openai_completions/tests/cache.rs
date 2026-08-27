//! Request builder: prompt-cache keys, retention and `cache_control` breakpoints.

use super::*;

#[test]
fn prompt_cache_key_for_openai_with_session() {
    let m = openai_model();
    let opts = StreamOptions {
        session_id: Some(cyrup_core::SessionId::from("sess-123")),
        ..Default::default()
    };
    let body = build_body(&m, &Context::default(), &opts);
    assert_eq!(body["prompt_cache_key"], "sess-123");
    // Together does not get a prompt_cache_key (no api.openai.com, short retention).
    let body = build_body(&model(), &Context::default(), &opts);
    assert!(body.get("prompt_cache_key").is_none());
}

// Gap 2: Pi `resolveCacheRetention` (openai-completions.ts:141-149) — when the caller did not
// set retention, `PI_CACHE_RETENTION == "long"` promotes to Long; an explicit value wins.
#[test]
fn pi_cache_retention_env_promotes_long() {
    use std::collections::BTreeMap;
    let m = openai_model();
    let mut env = BTreeMap::new();
    env.insert("PI_CACHE_RETENTION".to_string(), "long".to_string());

    // Unset caller retention + PI_CACHE_RETENTION=long (scoped overlay) => promoted to Long,
    // which (on api.openai.com, supportsLongCacheRetention) emits `prompt_cache_retention`.
    let opts = StreamOptions {
        cache_retention: None,
        ..Default::default()
    };
    let body = build_body_with_env(&m, &Context::default(), &opts, Some(&env)).unwrap();
    assert_eq!(body["prompt_cache_retention"], "24h");

    // Explicit caller value wins over env: Short stays Short (no 24h).
    let opts = StreamOptions {
        cache_retention: Some(CacheRetention::Short),
        ..Default::default()
    };
    let body = build_body_with_env(&m, &Context::default(), &opts, Some(&env)).unwrap();
    assert!(body.get("prompt_cache_retention").is_none());

    // resolve_cache_retention precedence, directly (overlay-driven, deterministic).
    assert_eq!(
        resolve_cache_retention(None, Some(&env)),
        CacheRetention::Long
    );
    assert_eq!(
        resolve_cache_retention(Some(CacheRetention::None), Some(&env)),
        CacheRetention::None
    );
    let empty = BTreeMap::new();
    assert_eq!(
        resolve_cache_retention(None, Some(&empty)),
        CacheRetention::Short
    );
}

// DRIFT-028 — pi `addCacheControlToLastConversationMessage` (openai-completions.ts:913-925
// @v0.83.0) walks backwards accepting `user`, `assistant` AND `tool`. cyrup dropped the `tool`
// arm, so in an agent loop (where the last message is almost always a tool result) the
// breakpoint landed one message too early on every turn.
#[test]
fn cache_breakpoint_lands_on_a_trailing_tool_result() {
    let cc = json!({"type": "ephemeral"});

    // Conversation ending in a tool result: the breakpoint is on THAT message.
    let mut messages = vec![
        json!({"role": "system", "content": "sys"}),
        json!({"role": "user", "content": "hi"}),
        json!({"role": "assistant", "content": "calling"}),
        json!({"role": "tool", "tool_call_id": "c1", "content": "tool output"}),
    ];
    add_cache_control_to_last_conversation_message(&mut messages, &cc);
    assert_eq!(
        messages[3]["content"][0]["cache_control"], cc,
        "the trailing tool result must carry the breakpoint"
    );
    assert!(
        messages[2].get("content").and_then(Value::as_array).is_none(),
        "the assistant message must be left as a plain string — untouched"
    );

    // Conversation ending in an assistant message is unchanged by the widening.
    let mut messages = vec![
        json!({"role": "user", "content": "hi"}),
        json!({"role": "assistant", "content": "answer"}),
    ];
    add_cache_control_to_last_conversation_message(&mut messages, &cc);
    assert_eq!(messages[1]["content"][0]["cache_control"], cc);
    assert_eq!(messages[0]["content"], json!("hi"));

    // A `system`/`developer` message is still never a conversation breakpoint: an empty
    // trailing tool result makes `addCacheControlToTextContent` return false and the walk
    // continues past it to the user turn, skipping the system prompt entirely.
    let mut messages = vec![
        json!({"role": "system", "content": "sys"}),
        json!({"role": "user", "content": "hi"}),
        json!({"role": "tool", "tool_call_id": "c1", "content": ""}),
    ];
    add_cache_control_to_last_conversation_message(&mut messages, &cc);
    assert_eq!(messages[1]["content"][0]["cache_control"], cc);
    assert_eq!(messages[2]["content"], json!(""));
    assert_eq!(messages[0]["content"], json!("sys"));
}
