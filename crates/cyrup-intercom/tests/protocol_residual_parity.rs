//! The five residual acceptance deltas found by the live pi-vs-cyrup parity table.
//!
//! Each rule is transcribed from the pi source it ports, not inferred:
//!   - `registered.features`  — v0.9.2 `broker/client.ts:395-400`
//!   - `extension_owner`      — v0.9.2 `broker/client.ts:539-548`
//!   - `extension_message`    — v0.9.2 `broker/client.ts:554-565`
//!   - `extension_state`      — v0.9.2 `broker/client.ts:571-578`
//!   - `extension_state_result` — v0.9.2 `broker/client.ts:585-590`
//!
//! Direction matters in both senses. cyrup LOOSER than pi is an input-validation hole on a socket
//! every local process can reach; cyrup STRICTER is a disconnect pi would never have had. Each
//! rejection case below is paired with an acceptance case that must stay green.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use cyrup_intercom::transport::protocol::BrokerMessage;

fn decode(raw: &str) -> Result<BrokerMessage, serde_json::Error> {
    serde_json::from_str::<BrokerMessage>(raw)
}

#[test]
fn registered_features_null_is_rejected_but_absent_and_array_are_accepted() {
    // pi: `features !== undefined && !Array.isArray(features)` -> throw. `typeof null === "object"`.
    assert!(
        decode(r#"{"type":"registered","sessionId":"s1","features":null}"#).is_err(),
        "explicit null features must be rejected, as pi throws `Invalid registered features`"
    );
    assert!(
        decode(r#"{"type":"registered","sessionId":"s1","features":[1,2]}"#).is_err(),
        "a non-string element must be rejected (`features.every(f => typeof f === \"string\")`)"
    );
    // MIRRORS — both are what a real pi broker sends.
    decode(r#"{"type":"registered","sessionId":"s1"}"#).expect("absent features is legal");
    decode(r#"{"type":"registered","sessionId":"s1","features":["extension-bus-v1"]}"#)
        .expect("pi's broker advertises exactly this");
}

#[test]
fn extension_owner_rejects_explicit_null_owner_fields() {
    assert!(
        decode(r#"{"type":"extension_owner","namespace":"ns","ownerId":null,"ownerEpoch":"e1"}"#)
            .is_err(),
        "pi: `ownerId !== undefined && !hasOwnerId` -> throw"
    );
    // MIRRORS: both present, and both absent, are the two legal shapes.
    decode(r#"{"type":"extension_owner","namespace":"ns","ownerId":"o1","ownerEpoch":"e1"}"#)
        .expect("both present is legal");
    decode(r#"{"type":"extension_owner","namespace":"ns"}"#).expect("both absent is legal");
}

#[test]
fn extension_state_revision_is_bounded_by_max_safe_integer() {
    // 2^53 - 1 is the last safe integer; 2^53 is not.
    assert!(
        decode(r#"{"type":"extension_state","namespace":"ns","revision":9007199254740992,"payload":{}}"#)
            .is_err(),
        "pi rejects a non-safe integer via Number.isSafeInteger"
    );
    assert!(
        decode(r#"{"type":"extension_state","namespace":"ns","revision":-1,"payload":{}}"#).is_err(),
        "pi requires revision >= 0"
    );
    // MIRROR: the boundary value itself must still be accepted.
    decode(r#"{"type":"extension_state","namespace":"ns","revision":9007199254740991,"payload":{}}"#)
        .expect("MAX_SAFE_INTEGER is itself safe");
}

#[test]
fn an_absent_payload_is_accepted_because_pi_never_checks_it() {
    // cyrup was STRICTER than pi here: a required `payload` destroyed a connection pi would serve.
    // Neither the `extension_state` nor the `extension_message` guard mentions `payload` at all.
    decode(r#"{"type":"extension_state","namespace":"ns","revision":1}"#)
        .expect("pi's extension_state guard never mentions payload");
    decode(r#"{"type":"extension_message","namespace":"ns","fromSessionId":"s1"}"#)
        .expect("pi's extension_message guard never mentions payload");
}

#[test]
fn extension_state_result_rejects_null_reason_and_unsafe_revision() {
    assert!(
        decode(
            r#"{"type":"extension_state_result","namespace":"ns","committed":false,"revision":1,"reason":null}"#
        )
        .is_err(),
        "pi: `reason !== undefined && typeof reason !== \"string\"` -> throw"
    );
    assert!(
        decode(
            r#"{"type":"extension_state_result","namespace":"ns","committed":false,"revision":9007199254740992}"#
        )
        .is_err(),
        "same safe-integer bound as extension_state"
    );
    // MIRROR: the shape pi's broker actually sends on a refused commit.
    decode(
        r#"{"type":"extension_state_result","namespace":"ns","committed":false,"revision":0,"reason":"Session has not advertised extension capability"}"#,
    )
    .expect("the real refusal frame must decode");
}

/// The owner XOR-presence rule, which had NO test until now.
///
/// pi requires `hasOwnerId === hasOwnerEpoch` on both `extension_owner`
/// (v0.9.2 `broker/client.ts:541-548`) and `extension_message` (`:557-565`): a namespace is either
/// owned — id AND epoch — or unowned, never half. An epoch without an id cannot be checked against
/// anything, and an id without an epoch cannot be invalidated when ownership moves, so a
/// half-populated record is an ownership claim nothing can revoke.
///
/// This is also the one part of the rebuilt `protocol.rs` whose FIELD LAYOUT was inferred rather
/// than pinned by a consumer: no surviving test covered it, and a cross-field rule cannot be
/// expressed by two independent `Option<String>` fields. Pinning it here so the layout can no
/// longer drift silently.
#[test]
fn owner_id_and_owner_epoch_must_be_both_present_or_both_absent() {
    for half in [
        r#"{"type":"extension_owner","namespace":"ns","ownerId":"o1"}"#,
        r#"{"type":"extension_owner","namespace":"ns","ownerEpoch":"e1"}"#,
        r#"{"type":"extension_message","namespace":"ns","fromSessionId":"s1","ownerId":"o1"}"#,
        r#"{"type":"extension_message","namespace":"ns","fromSessionId":"s1","ownerEpoch":"e1"}"#,
    ] {
        assert!(
            decode(half).is_err(),
            "half-populated ownership must be rejected (pi: `hasOwnerId !== hasOwnerEpoch` -> throw): {half}"
        );
    }

    // MIRRORS — the two legal shapes, on both tags. These must stay green, or the rule has become
    // "reject all ownership" rather than "reject half of it".
    for legal in [
        r#"{"type":"extension_owner","namespace":"ns","ownerId":"o1","ownerEpoch":"e1"}"#,
        r#"{"type":"extension_owner","namespace":"ns"}"#,
        r#"{"type":"extension_message","namespace":"ns","fromSessionId":"s1","ownerId":"o1","ownerEpoch":"e1"}"#,
        r#"{"type":"extension_message","namespace":"ns","fromSessionId":"s1"}"#,
    ] {
        decode(legal).unwrap_or_else(|e| panic!("a legal ownership shape must decode: {legal} -> {e}"));
    }
}
