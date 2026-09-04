//! SEAM-116 — the `clear_queue` RPC verb pi added in the v0.84.1→v0.84.4 window
//! (`a79b37334 feat(coding-agent): expose RPC queue clearing`, first tag v0.84.4).
//!
//! `modes/rpc/rpc-types.ts:26` @v0.84.4 adds `{ id?: string; type: "clear_queue" }` to `RpcCommand`
//! and `:124-128` the `{ command: "clear_queue"; success: true; data: { steering: string[];
//! followUp: string[] } }` reply; `rpc-mode.ts:433-435` is `return success(id, "clear_queue",
//! session.clearQueue())`, and `core/agent-session.ts:1588-1596` `clearQueue()` snapshots both
//! queues, empties them, emits `queue_update` and returns `{ steering, followUp }`. The shipped
//! docs (`packages/coding-agent/docs/rpc.md:137-158` @v0.84.4) name the use: *"To implement
//! interactive Esc behavior, send `clear_queue` before `abort`, then restore the returned text in
//! the client editor."*
//!
//! Each case here drives the real host (`run_rpc`) — the first over a scripted `Cursor` for the
//! byte shape, the other two through [`crate::RpcClient`] over a duplex pair so every reply is
//! awaited before the next request is written (concurrent verbs are dispatched into a
//! `FuturesUnordered`, so a raw stream cannot order `clear_queue` against a following `get_state`).

use std::io::Cursor;
use std::sync::Arc;

use super::support::{build_runtime, fixture, parse_lines, spawn_rpc_duplex, type_of};
use crate::{RpcClient, run_rpc};
use cyrup_provider::faux::FauxProvider;
use serde_json::json;

/// The wire shape: `data` is exactly `{steering, followUp}` — pi's two key names and no other
/// (`agent-session.ts:1595` `return { steering, followUp }`), the queued text verbatim — and the
/// host's `queue_update` after the drain carries two EMPTY arrays (`:1594` `_emitQueueUpdate()`
/// runs after both mirrors are cleared). Key ORDER is deliberately not asserted: every `data`
/// payload in this host is a `serde_json::Value`, whose object is a `BTreeMap` (the workspace
/// keeps `preserve_order` OFF on purpose — see the root `Cargo.toml`), so keys sort; JSON object
/// order carries no meaning and pi's own `getData` reads by name.
///
/// `steer`/`follow_up` are inline verbs, so they are fully applied before the following
/// `clear_queue` line is even read — the ordering this raw stream relies on.
#[tokio::test]
async fn clear_queue_answers_pi_s_steering_follow_up_shape_and_empties_the_queues() {
    let fx = fixture();
    let runtime = build_runtime(&fx, Arc::new(FauxProvider::new())).await;

    let input = concat!(
        r#"{"type":"steer","id":"s","message":"Change direction"}"#,
        "\n",
        r#"{"type":"follow_up","id":"f","message":"Summarize when finished"}"#,
        "\n",
        r#"{"type":"clear_queue","id":"cq"}"#,
        "\n",
    );
    let reader = Cursor::new(input.as_bytes().to_vec());
    let mut out: Vec<u8> = Vec::new();
    run_rpc(&runtime, reader, &mut out)
        .await
        .expect("rpc mode runs");
    let lines = parse_lines(&out);

    let reply = lines
        .iter()
        .find(|l| type_of(l) == "response" && l["command"] == "clear_queue")
        .expect("`clear_queue` must be a recognized verb (rpc-types.ts:26 @v0.84.4)");
    assert_eq!(reply["id"], "cq", "{reply}");
    assert_eq!(reply["success"], true, "{reply}");
    assert_eq!(
        reply["data"],
        json!({
            "steering": ["Change direction"],
            "followUp": ["Summarize when finished"],
        }),
        "pi's reply data is exactly {{steering, followUp}} (rpc-types.ts:124-128): {reply}"
    );
    // The queues really are empty afterwards: the LAST `queue_update` on the wire is the one the
    // drain emitted, and it carries nothing.
    let last_queue_update = lines
        .iter()
        .rev()
        .find(|l| type_of(l) == "queue_update")
        .expect("the drain emits a queue_update (agent-session.ts:1594)");
    assert_eq!(
        last_queue_update["steering"],
        json!([]),
        "{last_queue_update}"
    );
    assert_eq!(
        last_queue_update["followUp"],
        json!([]),
        "{last_queue_update}"
    );
    // And a queue_update carrying BOTH texts preceded it — the queues were non-empty going in, so
    // the empty snapshot above is the drain's doing rather than a vacuous initial state.
    assert!(
        lines.iter().any(|l| type_of(l) == "queue_update"
            && l["steering"] == json!(["Change direction"])
            && l["followUp"] == json!(["Summarize when finished"])),
        "a queue_update with both messages must precede the drain: {lines:?}"
    );
}

/// End to end through [`RpcClient::clear_queue`] (pi `rpc-client.ts:226-229` @v0.84.4): the typed
/// reply carries the queued text, `get_state` then reports `pendingMessageCount: 0`, and a second
/// `clear_queue` on the now-empty queues answers two empty arrays rather than an error — pi's
/// `clearQueue()` has no "nothing queued" failure path.
#[tokio::test]
async fn clear_queue_over_the_client_drains_then_get_state_shows_nothing_pending() {
    let fx = fixture();
    let runtime = build_runtime(&fx, Arc::new(FauxProvider::new())).await;
    let (client_tx, client_reader, host) = spawn_rpc_duplex(runtime);
    let client = RpcClient::attach(client_reader, client_tx);

    client.steer("Change direction", None).await.expect("steer");
    client
        .follow_up("Summarize when finished", None)
        .await
        .expect("follow_up");
    let before = client.get_state().await.expect("get_state");
    assert_eq!(before["pendingMessageCount"], json!(2), "{before}");

    let drained = client.clear_queue().await.expect("clear_queue");
    assert_eq!(drained.steering, vec!["Change direction".to_string()]);
    assert_eq!(
        drained.follow_up,
        vec!["Summarize when finished".to_string()]
    );

    let after = client.get_state().await.expect("get_state");
    assert_eq!(after["pendingMessageCount"], json!(0), "{after}");

    let again = client
        .clear_queue()
        .await
        .expect("clear_queue on empty queues");
    assert!(again.steering.is_empty(), "{:?}", again.steering);
    assert!(again.follow_up.is_empty(), "{:?}", again.follow_up);

    client.stop().await;
    host.await.expect("rpc loop exits at EOF");
}

/// The concurrency case the item named: with one message already queued, race `clear_queue`
/// against a `steer` from another task. Whichever order the host serves them in, the drained
/// snapshot and the post-clear residue must partition the two messages — nothing lost, nothing
/// duplicated — because the host drains through `AgentSession::drain_queue`, which takes both
/// mirrors under their guards in one pass rather than read-then-clear.
///
/// Run several rounds so both interleavings get a chance to occur; the assertion holds for either.
#[tokio::test]
async fn clear_queue_racing_a_steer_never_loses_or_duplicates_a_message() {
    let fx = fixture();
    let runtime = build_runtime(&fx, Arc::new(FauxProvider::new())).await;
    let (client_tx, client_reader, host) = spawn_rpc_duplex(runtime);
    let client = Arc::new(RpcClient::attach(client_reader, client_tx));

    for round in 0..8 {
        let first = format!("queued-before-{round}");
        let racing = format!("steered-during-{round}");
        client.steer(&first, None).await.expect("steer");

        let clear = tokio::spawn({
            let client = Arc::clone(&client);
            async move { client.clear_queue().await }
        });
        let steer = tokio::spawn({
            let client = Arc::clone(&client);
            let racing = racing.clone();
            async move { client.steer(&racing, None).await }
        });
        let drained = clear.await.expect("join").expect("clear_queue");
        steer.await.expect("join").expect("steer");
        let residue = client.clear_queue().await.expect("final clear_queue");

        // The first message was queued and acknowledged BEFORE the race began, so the drain
        // always carries it.
        assert_eq!(
            drained.steering.first(),
            Some(&first),
            "round {round}: drained={:?}",
            drained.steering
        );
        // The racing steer landed on exactly one side.
        let mut seen: Vec<&String> = drained.steering.iter().chain(&residue.steering).collect();
        seen.sort();
        let mut expected = vec![&first, &racing];
        expected.sort();
        assert_eq!(
            seen, expected,
            "round {round}: drained={:?} residue={:?}",
            drained.steering, residue.steering
        );
        assert!(drained.follow_up.is_empty() && residue.follow_up.is_empty());

        let state = client.get_state().await.expect("get_state");
        assert_eq!(state["pendingMessageCount"], json!(0), "{state}");
    }

    client.stop().await;
    host.await.expect("rpc loop exits at EOF");
}

/// The unit variant round-trips through the same `#[serde(other)]`-guarded enum as every other
/// verb: `{"type":"clear_queue"}` is `ClearQueue`, not `Unknown`.
#[test]
fn clear_queue_is_a_recognized_session_command() {
    let cmd: crate::SessionCommand =
        serde_json::from_str(r#"{"type":"clear_queue","id":"9"}"#).expect("parse clear_queue");
    assert!(
        matches!(cmd, crate::SessionCommand::ClearQueue),
        "expected ClearQueue, got {cmd:?}"
    );
}
