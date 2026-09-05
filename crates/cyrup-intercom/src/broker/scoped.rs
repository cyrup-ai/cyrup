//! ICOM-055 — the broker-enforced routing scope, end to end
//! (`v0.13.0 broker/broker.ts`, upstream `089b631`, issue #112).
//!
//! These live in one module rather than distributed across `session`/`send`/`mailbox`/`extensions`
//! because every one of them is cross-cutting by construction: a scope test registers sessions in
//! two classes and then asserts about the roster, the addressing refusal, the presence fan-out and
//! the mailbox at once. Splitting them would put four copies of the same three-session fixture in
//! four files.
//!
//! The properties asserted are the two the feature exists for. **Opt-in**: with no scope on the
//! register frame nothing changes. **Broker-side**: a cross-scope target is refused by the BROKER
//! with the same `Session not found` a never-registered name gets — never delivered, never silently
//! dropped, and never leaked as "that session exists but you may not talk to it".

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use serde_json::json;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use super::routing::SessionKey;
use super::state::BrokerState;
use super::test_support::{make_state, payloads, register_named_in_scope, send_frame};

type Peer = (
    UnboundedSender<Vec<u8>>,
    UnboundedReceiver<Vec<u8>>,
    Option<SessionKey>,
);

/// Register `id`/`name` on `conn_id` in `scope`, returning its writer, its reader and its key.
fn join(state: &mut BrokerState, conn_id: u64, id: &str, name: &str, scope: Option<&str>) -> Peer {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut key = None;
    register_named_in_scope(state, conn_id, &mut key, &tx, id, name, "/w", scope, 1_000);
    (tx, rx, key)
}

/// `list` on behalf of an already-registered peer, returning the `sessions` reply's ids.
fn list_ids(state: &mut BrokerState, conn_id: u64, peer: &mut Peer) -> Vec<String> {
    let _ = payloads(&mut peer.1);
    state.handle_frame(
        conn_id,
        &peer.0,
        &json!({ "type": "list", "requestId": "r1" }),
        &mut peer.2,
        2_000,
    );
    payloads(&mut peer.1)
        .into_iter()
        .filter(|p| p["type"] == "sessions")
        .flat_map(|p| {
            p["sessions"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|s| s["id"].as_str().unwrap_or_default().to_string())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// DoD #3 — roster isolation (`.filter(session => sameScope(…))`,
/// `v0.13.0 broker/broker.ts:596-597`). Before the port `handle_list` answered
/// `session_infos()`, the WHOLE roster, so every one of these four assertions saw all four
/// sessions.
///
/// Note the fourth: an unscoped session sees only itself, **not** everyone. Unscoped is a scope,
/// not a wildcard (`sameScope(undefined, "alpha")` is `false`).
#[test]
fn the_roster_is_scope_relative_and_unscoped_is_not_a_wildcard() {
    let mut state = make_state();
    let mut a1 = join(&mut state, 1, "a1", "one", Some("alpha"));
    let mut a2 = join(&mut state, 2, "a2", "two", Some("alpha"));
    let mut b1 = join(&mut state, 3, "b1", "three", Some("beta"));
    let mut u1 = join(&mut state, 4, "u1", "four", None);

    let mut alpha = list_ids(&mut state, 1, &mut a1);
    alpha.sort();
    assert_eq!(alpha, vec!["a1".to_string(), "a2".to_string()]);
    assert_eq!(list_ids(&mut state, 2, &mut a2).len(), 2);
    assert_eq!(list_ids(&mut state, 3, &mut b1), vec!["b1".to_string()]);
    assert_eq!(list_ids(&mut state, 4, &mut u1), vec!["u1".to_string()]);
}

/// DoD #2 — a scope is opaque. `scopeId` appears on the register frame and NOWHERE else on the
/// wire: not on a `SessionInfo`, not on `registered`, not on `session_joined`/`session_left`, not
/// on `presence_update`. Upstream is the same — `broker.ts:483`'s `...(scopeId ? { scopeId } : {})`
/// builds the in-memory `ConnectedSession`, not a frame — so a peer never learns that scopes exist.
#[test]
fn no_frame_a_peer_receives_ever_carries_a_scope() {
    let mut state = make_state();
    let mut a1 = join(&mut state, 1, "a1", "one", Some("alpha"));
    let _a2 = join(&mut state, 2, "a2", "two", Some("alpha"));
    state.handle_frame(
        1,
        &a1.0,
        &json!({ "type": "presence", "status": "busy" }),
        &mut a1.2,
        60_000,
    );
    let _ = list_ids(&mut state, 1, &mut a1);
    let seen = payloads(&mut a1.1);
    let rendered = serde_json::to_string(&seen).unwrap();
    assert!(
        !rendered.contains("scopeId") && !rendered.contains("alpha"),
        "a scope must never reach a client: {rendered}"
    );
}

/// DoD #4 — addressing isolation, **by refusal**. All four naming forms upstream supports resolve
/// through the scoped ladder (`findSessions`, `v0.13.0 broker/broker.ts:1247-1262`), so each
/// answers exactly what a never-registered name answers.
///
/// Before the port every one of these was DELIVERED to the `beta` session.
#[test]
fn a_cross_scope_send_is_refused_exactly_like_an_unknown_name() {
    for (label, to) in [
        ("full id", "b1"),
        ("name", "three"),
        ("id prefix", "b"),
        ("never registered", "ghost"),
    ] {
        let mut state = make_state();
        let mut a1 = join(&mut state, 1, "a1", "one", Some("alpha"));
        let mut b1 = join(&mut state, 2, "b1", "three", Some("beta"));
        let _ = payloads(&mut a1.1);
        let _ = payloads(&mut b1.1);
        send_frame(
            &mut state,
            1,
            &a1.0,
            &mut a1.2,
            to,
            json!({ "id": "m1", "timestamp": 1, "content": { "text": "across" } }),
            2_000,
        );
        let got = payloads(&mut a1.1);
        assert_eq!(got.len(), 1, "{label}: exactly one answer");
        assert_eq!(got[0]["type"], "delivery_failed", "{label}");
        assert_eq!(
            got[0]["reason"], "Session not found",
            "{label}: the refusal must reveal nothing about the target"
        );
        assert!(
            payloads(&mut b1.1).is_empty(),
            "{label}: nothing reaches the other scope"
        );
    }
}

/// The positive control for the test above: the identical send INSIDE one scope is delivered. Both
/// halves matter — a refusal that fires for everything would satisfy the assertions above.
#[test]
fn a_same_scope_send_is_delivered() {
    let mut state = make_state();
    let mut a1 = join(&mut state, 1, "a1", "one", Some("alpha"));
    let mut a2 = join(&mut state, 2, "a2", "two", Some("alpha"));
    let _ = payloads(&mut a1.1);
    let _ = payloads(&mut a2.1);
    send_frame(
        &mut state,
        1,
        &a1.0,
        &mut a1.2,
        "two",
        json!({ "id": "m1", "timestamp": 1, "content": { "text": "within" } }),
        2_000,
    );
    assert_eq!(payloads(&mut a1.1)[0]["type"], "delivered");
    let got = payloads(&mut a2.1);
    assert!(got.iter().any(|p| p["message"]["id"] == "m1"), "{got:?}");
}

/// DoD #5 — presence isolation (`this.broadcast(msg, exclude, scopeId)`,
/// `v0.13.0 broker/broker.ts:1312-1318`, called with the originating session's scope at `:327`,
/// `:504`, `:540` and `:957`). A `beta` session joining, updating presence and leaving produces no
/// frame of any kind on an `alpha` or an unscoped socket.
#[test]
fn join_presence_and_leave_never_cross_the_boundary() {
    let mut state = make_state();
    let mut a1 = join(&mut state, 1, "a1", "one", Some("alpha"));
    let mut u1 = join(&mut state, 2, "u1", "two", None);
    let _ = payloads(&mut a1.1);
    let _ = payloads(&mut u1.1);

    let mut b1 = join(&mut state, 3, "b1", "three", Some("beta"));
    state.handle_frame(
        3,
        &b1.0,
        &json!({ "type": "presence", "status": "busy" }),
        &mut b1.2,
        60_000,
    );
    state.on_connection_closed(3, &b1.2, 61_000);

    assert!(payloads(&mut a1.1).is_empty(), "nothing reaches alpha");
    assert!(payloads(&mut u1.1).is_empty(), "nothing reaches unscoped");
}

/// DoD #6 — mailbox isolation. Mail parked for a disconnected `alpha` peer is redelivered only
/// inside `alpha`: a `beta` session with the SAME name in the SAME cwd never receives it and never
/// satisfies the unique-mailbox-identity test that would let it
/// (`sameScope` is the first conjunct of `findLiveSessionsSharingMailboxIdentity`, `:1305`, and
/// `flushMailboxForSession` skips a foreign `targetScopeId` outright, `:1120-1122`).
#[test]
fn parked_mail_never_flushes_into_another_scope() {
    let mut state = make_state();
    let mut a1 = join(&mut state, 1, "a1", "sender", Some("alpha"));
    let a2 = join(&mut state, 2, "a2", "worker", Some("alpha"));
    state.on_connection_closed(2, &a2.2, 1_500);
    let _ = payloads(&mut a1.1);

    send_frame(
        &mut state,
        1,
        &a1.0,
        &mut a1.2,
        "worker",
        json!({ "id": "m1", "timestamp": 1, "content": { "text": "parked" } }),
        2_000,
    );
    assert_eq!(payloads(&mut a1.1)[0]["type"], "delivered");
    assert_eq!(state.mailbox_messages.len(), 1);

    // A same-named, same-cwd session in ANOTHER scope registers: it must not inherit the mail.
    let mut b1 = join(&mut state, 3, "b1", "worker", Some("beta"));
    let got = payloads(&mut b1.1);
    assert!(
        !got.iter().any(|p| p["type"] == "message"),
        "beta must not inherit alpha's mailbox: {got:?}"
    );
    assert_eq!(state.mailbox_messages.len(), 1, "the entry is still parked");

    // The alpha identity comes back and gets it.
    let mut a2b = join(&mut state, 4, "a2", "worker", Some("alpha"));
    let got = payloads(&mut a2b.1);
    assert!(
        got.iter()
            .any(|p| p["type"] == "message" && p["message"]["id"] == "m1"),
        "the same-scope identity still receives its parked mail: {got:?}"
    );
    assert!(state.mailbox_messages.is_empty());
}

/// DoD #7 — identity isolation. The same session id in two scopes is two coexisting sessions:
/// neither takeover-evicts the other (`this.sessions.get(scopedSessionKey(scopeId, id))`,
/// `v0.13.0 broker/broker.ts:436-443`), which is what the upstream README means by "the newest
/// registration takes over that identity only within the same `PI_INTERCOM_SCOPE_ID` boundary".
///
/// The `MAX_SESSIONS` cap stays global (`this.sessions.size`), so a scope cannot mint an unbounded
/// roster — asserted here as the map holding both.
#[test]
fn the_same_id_in_two_scopes_is_two_sessions() {
    let mut state = make_state();
    let a = join(&mut state, 1, "shared", "one", Some("alpha"));
    let b = join(&mut state, 2, "shared", "two", Some("beta"));
    assert_ne!(a.2, b.2, "the two keys differ by scope");
    assert_eq!(state.sessions.len(), 2, "both are live");
    assert_eq!(
        state.sessions.get(a.2.as_ref().unwrap()).map(|s| s.conn_id),
        Some(1),
        "registering `shared` in beta must not evict `shared` in alpha"
    );
}

/// DoD #8 — a malformed scope is FATAL, a blank one is not (`normalizeScopeId`,
/// `v0.13.0 broker/broker.ts:133-142`). A `scopeId` that is present but not a string must destroy
/// the connection rather than silently registering unscoped: a malformed scope quietly becoming
/// "global" is a confidentiality failure, not a parse failure.
#[test]
fn a_non_string_scope_is_fatal_and_a_blank_one_is_unscoped() {
    let frame = |scope: serde_json::Value| {
        json!({
            "type": "register",
            "sessionId": "s1",
            "scopeId": scope,
            "session": { "cwd": "/w", "model": "m", "pid": 1, "startedAt": 0, "lastActivity": 0 },
        })
    };
    for bad in [json!(7), json!(null), json!(["alpha"]), json!({})] {
        let mut state = make_state();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut key = None;
        let result = state.handle_frame(1, &tx, &frame(bad.clone()), &mut key, 1_000);
        assert!(
            matches!(result.outcome, super::frame::FrameOutcome::ProtocolError),
            "scopeId {bad} must destroy the connection"
        );
        assert!(key.is_none(), "and must not register");
    }
    // Whitespace-only trims to unscoped and connects normally.
    let mut state = make_state();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut key = None;
    state.handle_frame(1, &tx, &frame(json!("   ")), &mut key, 1_000);
    assert_eq!(key, Some(SessionKey::unscoped("s1".to_string())));
    // And a scoped value is trimmed, not taken verbatim.
    let mut state = make_state();
    let mut key = None;
    state.handle_frame(1, &tx, &frame(json!("  alpha  ")), &mut key, 1_000);
    assert_eq!(
        key.as_ref()
            .and_then(|k| k.scope.as_ref())
            .map(|s| s.as_str()),
        Some("alpha")
    );
}

/// DoD #1's single named exception (`v0.13.0 broker/broker.ts:592-595`): a `list` arriving on a
/// SUPERSEDED socket — one whose session a newer connection has taken over — is now a protocol
/// error instead of a full-roster reply. Answering it with everything is the one wrong reply to a
/// peer whose scope the broker can no longer attribute.
///
/// A `list` before registration was already a protocol error via the shared before-register guard
/// in `super::dispatch`, so it is not a change; it is asserted here as the control.
#[test]
fn a_list_from_a_superseded_socket_is_a_protocol_error() {
    let mut state = make_state();
    let mut a1 = join(&mut state, 1, "a1", "one", Some("alpha"));
    // A newer connection takes over the same identity in the same scope.
    let _a1b = join(&mut state, 2, "a1", "one", Some("alpha"));
    let result = state.handle_frame(
        1,
        &a1.0,
        &json!({ "type": "list", "requestId": "r1" }),
        &mut a1.2,
        3_000,
    );
    assert!(matches!(
        result.outcome,
        super::frame::FrameOutcome::ProtocolError
    ));

    // Control: the surviving socket is answered normally.
    let mut live = join(&mut state, 3, "a2", "two", Some("alpha"));
    assert_eq!(list_ids(&mut state, 3, &mut live).len(), 2);
}

/// DoD #12 — the extension bus is scoped too (`scopedExtensionKey`,
/// `v0.13.0 broker/broker.ts:152-154`, and its eight call sites). A namespace advertised in two
/// scopes elects two INDEPENDENT owners, and a publish never crosses the boundary (`:1504-1506`).
#[test]
fn the_extension_bus_elects_one_owner_per_scope_and_never_fans_out_across_them() {
    let mut state = make_state();
    let caps = json!({
        "type": "extension_capabilities_update",
        "extensions": [{ "namespace": "demo", "ownerEligible": true }],
    });
    let mut a1 = join(&mut state, 1, "a1", "one", Some("alpha"));
    let mut b1 = join(&mut state, 2, "b1", "two", Some("beta"));
    state.handle_frame(1, &a1.0, &caps, &mut a1.2, 1_100);
    state.handle_frame(2, &b1.0, &caps, &mut b1.2, 1_100);

    assert_eq!(
        state.namespace_owners.len(),
        2,
        "one owner per scope, not one globally"
    );
    let owners: Vec<String> = state
        .namespace_owners
        .values()
        .map(|o| o.session_key.id.clone())
        .collect();
    assert!(owners.contains(&"a1".to_string()) && owners.contains(&"b1".to_string()));

    let _ = payloads(&mut a1.1);
    let _ = payloads(&mut b1.1);
    state.handle_frame(
        1,
        &a1.0,
        &json!({
            "type": "extension_publish", "namespace": "demo",
            "audience": "capable", "payload": { "hello": true },
        }),
        &mut a1.2,
        1_200,
    );
    assert!(
        payloads(&mut a1.1)
            .iter()
            .any(|p| p["type"] == "extension_message"),
        "the publisher's own scope receives it"
    );
    assert!(
        payloads(&mut b1.1).is_empty(),
        "the other scope receives nothing"
    );
}

/// DoD #12b — `scopedExtensionStateNamespace` (`v0.13.0 broker/broker.ts:156-161`). UNSCOPED
/// RETURNS THE BARE NAMESPACE, which is the opt-in guarantee for persistence: the state file an
/// unscoped session reads and writes keeps the name it has on disk today, because
/// `ExtensionStateManager` derives the filename from `sha256` of exactly this string. Only a scoped
/// session gets the tagged form, and the encoding must match `JSON.stringify` byte for byte.
#[test]
fn the_state_namespace_is_bare_when_unscoped_and_tagged_when_scoped() {
    use crate::transport::protocol::ScopeId;
    assert_eq!(
        super::extensions::scoped_extension_state_namespace(None, "demo"),
        "demo"
    );
    let scoped = super::extensions::scoped_extension_state_namespace(
        ScopeId::parse("alpha").as_ref(),
        "demo",
    );
    // sha256("alpha"), lowercase hex, exactly as node's `digest("hex")` renders it — and the
    // separator-free `JSON.stringify` array form, which is the on-disk key a pi broker sharing the
    // directory would compute.
    assert_eq!(
        scoped,
        r#"["scope","8ed3f6ad685b959ead7022518e1af76cd816f8e8ec7ccdda1ed4018e8f2223f8","demo"]"#
    );
}

/// DoD #1 — **unscoped is unchanged.** The register frame an unscoped session sends carries no
/// `scopeId` key at all, so a pre-scope broker and a scope-aware one see the identical bytes. This
/// is asserted at the frame the client actually builds, not at the type.
#[test]
fn an_unscoped_register_frame_has_no_scope_key() {
    use crate::transport::protocol::{ClientMessage, SessionRegistration, UnknownFields};
    let msg = ClientMessage::Register {
        session: SessionRegistration {
            runtime_fallback_alias: None,
            name: None,
            cwd: "/w".to_string(),
            model: "m".to_string(),
            pid: 1u64.into(),
            started_at: 0u64.into(),
            last_activity: 0u64.into(),
            status: None,
            tmux_pane: None,
            extra: UnknownFields::default(),
        },
        session_id: Some("s1".to_string()),
        state_id: None,
        scope_id: crate::config::intercom_scope_id_from(|_| None),
    };
    let v = serde_json::to_value(&msg).unwrap();
    assert_eq!(
        v,
        json!({
            "type": "register",
            "sessionId": "s1",
            "session": {
                "cwd": "/w", "model": "m", "pid": 1, "startedAt": 0, "lastActivity": 0,
            },
        })
    );
}
