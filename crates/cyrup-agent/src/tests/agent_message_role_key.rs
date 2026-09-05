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

use crate::AgentMessage;
use cyrup_core::AssistantMessage;
use std::sync::Arc;

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
    let raw = serde_json::to_string(&AgentMessage::Assistant(Arc::new(assistant()))).unwrap();
    assert_eq!(
        raw.matches(r#""role""#).count(),
        1,
        "the `role` discriminant must appear exactly once; a duplicate key is silently \
         last-wins in JSON.parse and a hard error in stricter parsers: {raw}"
    );
    assert!(
        raw.starts_with(r#"{"role":"assistant","#),
        "role stays first: {raw}"
    );
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
                details: None,
                display: true,
                timestamp: Some(7),
            },
            "custom",
        ),
    ] {
        let raw = serde_json::to_string(&msg).unwrap();
        assert_eq!(
            raw.matches(r#""role""#).count(),
            1,
            "exactly one role: {raw}"
        );
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["role"], expect, "{raw}");
    }
}

#[test]
fn every_arm_still_round_trips() {
    // MIRROR: removing the outer tag must not break `Deserialize`, which reads `role`. If this
    // regressed, transcripts would stop loading. SESS-043 replaced the derived `Deserialize` with a
    // hand-written one so the three declaration-merged roles can share one `App` arm; the four
    // typed arms below are the check that it stayed byte-compatible with the derive.
    for msg in [
        AgentMessage::Assistant(Arc::new(assistant())),
        AgentMessage::user_text("hello"),
        AgentMessage::Custom {
            kind: "note".into(),
            payload: serde_json::json!({"a": 1}),
            details: None,
            display: true,
            timestamp: Some(7),
        },
    ] {
        let raw = serde_json::to_string(&msg).unwrap();
        let back: AgentMessage =
            serde_json::from_str(&raw).unwrap_or_else(|e| panic!("must round-trip: {raw} -> {e}"));
        assert_eq!(back, msg, "round-trip must preserve the value: {raw}");
    }
}

#[test]
fn a_user_message_keeps_its_null_timestamp_key() {
    // The derive emitted `"timestamp":null` for `None` (no skip_serializing_if). Preserve that —
    // this test exists so a future "tidy-up" cannot silently change the wire shape while the
    // duplicate-role fix is credited for it.
    let raw = serde_json::to_string(&AgentMessage::user_text("hi")).unwrap();
    assert!(
        raw.contains(r#""timestamp":null"#),
        "null timestamp key retained: {raw}"
    );
}

/// SESS-043 — a declaration-merged coding-agent role rides through `AgentMessage::App` as its pi
/// wire object and re-emits every field of it, `role` included and still exactly once.
///
/// **Coverage, not proof:** the `App` variant is new in this change, so no pre-fix form of this
/// test can be red. What it pins is the property the layering depends on — this crate never parses
/// the payload, so no field may be added, dropped or rewritten in transit (`cyrup-session` owns the
/// shapes; see `coding-agent/src/core/messages.ts:68-77` @v0.83.0).
///
/// **Field ORDER is deliberately not asserted, and the limit is real rather than cosmetic.**
/// `serde_json::Map` is a `BTreeMap` unless the `preserve_order` feature is on, so parsing into
/// `App` already sorts the keys and no serializer can put pi's declaration order back. That is why
/// `App` is confined to the in-memory transcript: the two places byte-shape is load-bearing —
/// the session file and the extension `session_before_compact` payload — both go through
/// `cyrup_session::agent_message::AgentMessage`'s hand-written `SerializeMap`, which emits pi's
/// declaration order explicitly and never sees an `App`.
#[test]
fn an_app_message_round_trips_its_pi_wire_object() {
    let wire =
        r#"{"role":"compactionSummary","summary":"did the thing","tokensBefore":41,"timestamp":9}"#;
    let msg: AgentMessage = serde_json::from_str(wire).expect("a merged role parses");
    let AgentMessage::App { ref role, .. } = msg else {
        panic!("a merged role must land in App, got {msg:?}");
    };
    assert_eq!(*role, crate::AppRole::CompactionSummary);
    let raw = serde_json::to_string(&msg).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&raw).unwrap(),
        serde_json::from_str::<serde_json::Value>(wire).unwrap(),
        "every field survives the crossing unchanged: {raw}"
    );
    assert_eq!(
        raw.matches(r#""role""#).count(),
        1,
        "still exactly one role key: {raw}"
    );
}

/// The `App` fallback is closed over pi's four merged roles (`custom` has its own typed arm), so a
/// genuinely unknown role still fails to parse exactly as it did before `App` existed. Widening
/// that tolerance would turn a corrupt transcript into a silently-empty turn.
#[test]
fn an_unknown_role_is_still_a_parse_error() {
    let err = serde_json::from_str::<AgentMessage>(r#"{"role":"nonsense","x":1}"#);
    assert!(
        err.is_err(),
        "an unrecognized role must not be swallowed: {err:?}"
    );
}

/// SUBA-094 — a custom message's `display` must survive the wire in BOTH directions.
///
/// The `Custom` arm is the one surface an extension constructs directly, and pi carries `display`
/// ON the message (`coding-agent/src/core/messages.ts:50` @v0.84.4) precisely so every consumer —
/// the interactive host's `if (message.display)` gate at `interactive-mode.ts:3609`, and the
/// `appendCustomMessageEntry` that persists it — reads one value. cyrup's TUI reads this arm
/// through the serialized projection (`AgentMessage` is only a dev-dependency there), so a `display`
/// that does not reach the bytes cannot be honoured at all.
///
/// Written as a JSON→JSON round trip rather than over a constructed value so it exercises the
/// hand-written `Deserialize`/`Serialize` pair, which is where the field could be dropped.
#[test]
fn a_custom_message_round_trips_display_in_both_directions() {
    for want in [false, true] {
        let wire = serde_json::json!({
            "role": "custom",
            "kind": "subagent-notify",
            "payload": "Background run finished",
            "display": want,
            "timestamp": 7,
        });
        let msg: AgentMessage =
            serde_json::from_value(wire.clone()).expect("the custom arm parses its own wire form");
        assert!(
            matches!(&msg, AgentMessage::Custom { display, .. } if *display == want),
            "display survives Deserialize: {wire}"
        );
        let back = serde_json::to_value(&msg).expect("serializes");
        assert_eq!(
            back["display"], want,
            "display survives Serialize — the projection the TUI gates on: {back}"
        );
    }
}

/// An absent `display` (a producer predating the field) still parses, and parses as VISIBLE — the
/// disposition every such message already had.
#[test]
fn a_custom_message_without_display_defaults_to_visible() {
    let msg: AgentMessage = serde_json::from_value(serde_json::json!({
        "role": "custom",
        "kind": "note",
        "payload": "hi",
    }))
    .expect("an absent display is not a parse error");
    assert!(
        matches!(&msg, AgentMessage::Custom { display, .. } if *display),
        "absent display reads as visible"
    );
}
