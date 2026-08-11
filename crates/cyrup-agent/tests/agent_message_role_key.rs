//! `AgentMessage` must emit the `role` discriminant EXACTLY ONCE.
//!
//! A derived internally-tagged `Serialize` writes `role` itself and then delegates the `Assistant`
//! newtype payload to `AssistantMessage`, whose serializer self-tags — producing
//! `{"role":"assistant","role":"assistant",…}`. JSON permits duplicate keys syntactically, so
//! nothing ever errored: `JSON.parse` keeps the last silently, and stricter parsers reject the whole
//! document. `cyrup_core::Message` had the identical defect and fixed it with a manual serializer;
//! this wrapper did not, and it reaches `--json` stdout, RPC stdout and the transcript.
//!
//! Every pre-existing assertion looked at PARSED values, where a duplicate key is invisible. These
//! assert on the RAW BYTES, which is the only place the bug is observable.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use cyrup_agent::AgentMessage;
use cyrup_core::AssistantMessage;

fn assistant() -> AssistantMessage {
    serde_json::from_value(serde_json::json!({
        "role": "assistant", "content": [], "api": "faux", "provider": "faux", "model": "m",
        "usage": {"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0,
                  "cost":{"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0,"total":0.0}},
        "stopReason": "stop", "timestamp": 0
    }))
    .expect("the fixture is a valid pi-shaped assistant message")
}

#[test]
fn an_assistant_message_emits_role_exactly_once() {
    let raw = serde_json::to_string(&AgentMessage::Assistant(assistant())).unwrap();
    assert_eq!(
        raw.matches(r#""role""#).count(),
        1,
        "the `role` discriminant must appear exactly once; a duplicate key is silently \
         last-wins in JSON.parse and a hard error in stricter parsers: {raw}"
    );
    assert!(raw.starts_with(r#"{"role":"assistant","#), "role stays first: {raw}");
}

#[test]
fn the_other_arms_are_byte_unchanged_and_still_tagged() {
    // MIRROR: the fix must not have altered the arms that were already correct. Each must still
    // carry exactly one `role`, with the value the derive produced.
    for (msg, expect) in [
        (AgentMessage::user_text("hi"), "user"),
        (
            AgentMessage::Custom {
                kind: "note".into(),
                payload: serde_json::json!({"a": 1}),
                timestamp: Some(7),
            },
            "custom",
        ),
    ] {
        let raw = serde_json::to_string(&msg).unwrap();
        assert_eq!(raw.matches(r#""role""#).count(), 1, "exactly one role: {raw}");
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["role"], expect, "{raw}");
    }
}

#[test]
fn every_arm_still_round_trips() {
    // MIRROR: removing the outer tag must not break `Deserialize`, which is still derived and
    // reads `role`. If this regressed, transcripts would stop loading.
    for msg in [
        AgentMessage::Assistant(assistant()),
        AgentMessage::user_text("hello"),
        AgentMessage::Custom {
            kind: "note".into(),
            payload: serde_json::json!({"a": 1}),
            timestamp: Some(7),
        },
    ] {
        let raw = serde_json::to_string(&msg).unwrap();
        let back: AgentMessage = serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("must round-trip: {raw} -> {e}"));
        assert_eq!(back, msg, "round-trip must preserve the value: {raw}");
    }
}

#[test]
fn a_user_message_keeps_its_null_timestamp_key() {
    // The derive emitted `"timestamp":null` for `None` (no skip_serializing_if). Preserve that —
    // this test exists so a future "tidy-up" cannot silently change the wire shape while the
    // duplicate-role fix is credited for it.
    let raw = serde_json::to_string(&AgentMessage::user_text("hi")).unwrap();
    assert!(raw.contains(r#""timestamp":null"#), "null timestamp key retained: {raw}");
}
