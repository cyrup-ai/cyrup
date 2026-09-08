//! The three assertions that need a real `ConnectionTo<Client>`/`Responder`.
//!
//! `Responder` has a private constructor (agent-client-protocol-2.1.0 `src/jsonrpc.rs:4536`) and
//! `ConnectionTo` comes only out of `connect_to`, so none of these is expressible as a unit test at
//! any price. They are `ACP-212`'s rebuild-and-evict, `ACP-217`'s cancel-during-replay and
//! `ACP-005`'s dispose — the three items left open for exactly that reason once the mechanisms
//! themselves were closed.
//!
//! The harness is `tests/support/mod.rs`: a real [`cyrup_acp::serve`] over an in-memory transport,
//! with a host shaped like the binary's. Offline, deterministic, no spawned process.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod support;

use cyrup_core::StopReason;
use cyrup_provider::faux::{faux_assistant_message, faux_text};
use serde_json::json;
use support::{
    Harness, in_local_set, index_of_response, index_of_update, is_response_to, seed_session,
    updates_of, wait_for,
};

/// **ACP-212** — `session/load` on the ALREADY-LIVE id still rebuilds, still evicts the outgoing
/// runtime, and still re-advertises the command menu.
///
/// The rule is `ACP-225`'s, taken once and asserted both ways: `session/prompt` short-circuits on
/// live ([`cyrup_acp::RestoreGate::enter`]), `session/load` does not
/// ([`cyrup_acp::RestoreGate::rebuild`], `sessions.rs:935-949`). The existing
/// `prompt_short_circuits_on_live_and_load_does_not` asserts the GATE over `u32` stand-ins, because
/// `AgentSessionRuntime` has no constructor short of a real provider-backed build; this asserts the
/// whole path — a real host, a real build, a real eviction, over the wire.
///
/// # The eviction is a `session_shutdown{reason:"quit"}`, not a `SessionReplaced`
///
/// That is the recorded `[CYRUP-DELTA]` on [`cyrup_acp::SessionManager::install`]
/// (`sessions.rs:1627-1650`): `notify_replaced` has exactly one caller in the workspace —
/// `AgentSessionRuntime::install_inner`, the runtime replacing its OWN session — and the ACP host
/// never routes through it, because `AcpHost::build_runtime` builds a *fresh* runtime per session.
/// So there is no generation bump to carry a `SessionReplaced`, and the eviction is
/// `previous.runtime.dispose()`. This test asserts the behaviour cyrup actually has and names why,
/// rather than asserting the unit's literal wording against a path that cannot produce it.
///
/// # The prompt first is not scene-setting
///
/// A session's `*.jsonl` does not exist until its first assistant message (`persist_last` reaches
/// `store.create_exclusive` only once `has_assistant_message()` is true), so without it the load
/// hands `SessionTarget::Resume(<a path that is not there>)` to the factory.
#[test]
fn session_load_on_the_live_id_rebuilds_once_and_evicts_the_outgoing_runtime() {
    in_local_set(|| async {
        let mut h = Harness::start(vec![faux_assistant_message(
            vec![faux_text("hi")],
            StopReason::Stop,
        )])
        .await;
        let session = h.open_session().await;
        assert_eq!(h.host.builds(), 1, "session/new must build exactly once");

        // Give the session a file on disk.
        let id = h
            .client
            .request(
                "session/prompt",
                json!({"sessionId": session, "prompt": [{"type": "text", "text": "hello"}]}),
            )
            .await;
        let settled = h.client.response_to(id).await;
        assert_eq!(settled["result"]["stopReason"], "end_turn", "{settled}");
        assert_eq!(
            h.host.builds(),
            1,
            "ACP-225: a prompt for the LIVE session must short-circuit rather than rebuild"
        );
        assert!(
            h.host.shutdowns().is_empty(),
            "a prompt disposed something: {:?}",
            h.host.shutdowns()
        );

        // The load, on the id that is already live.
        let load = h
            .client
            .request(
                "session/load",
                json!({"sessionId": session, "cwd": h.cwd.path().to_string_lossy(),
                       "mcpServers": []}),
            )
            .await;
        let mut frames = h.client.drain_until(|v| is_response_to(v, load)).await;

        let load_at = index_of_response(&frames, load);
        assert!(
            frames[load_at].get("error").is_none(),
            "session/load failed: {}",
            frames[load_at]
        );
        assert!(
            frames[load_at]["result"]["modes"]["availableModes"].is_array(),
            "ACP-062: a load carries the same one-read surface a new session does: {}",
            frames[load_at]
        );

        assert_eq!(
            h.host.builds(),
            2,
            "ACP-212/ACP-225: session/load on the live id must NOT short-circuit — the host was \
             asked to build {} time(s) in total",
            h.host.builds()
        );

        // The eviction. `install` disposes the OUTGOING runtime (build 0) and never the incoming
        // one (build 1). The event travels a spawned drain task, so it is polled for rather than
        // read synchronously.
        assert!(
            wait_for(|| h.host.shutdowns().len() == 1).await,
            "ACP-212: expected exactly one eviction, saw {:?}",
            h.host.shutdowns()
        );
        assert_eq!(
            h.host.shutdowns(),
            vec![(0, "quit".to_string())],
            "ACP-212: the OUTGOING runtime is disposed with reason `quit`; the incoming one is not"
        );

        // ...and the menu is re-advertised, after the response (`ACP-217`'s follow-up half).
        frames.extend(
            h.client
                .drain_until(|v| {
                    v["params"]["update"]["sessionUpdate"] == "available_commands_update"
                })
                .await,
        );
        assert!(
            index_of_response(&frames, load) < index_of_update(&frames, "available_commands_update"),
            "ACP-212: a load must re-advertise commands, and after its own response"
        );
    });
}

/// **ACP-217**, second half — a `session/cancel` sent during a long `session/load` is observed
/// before the load's response.
///
/// This is `ACP-Q35`'s decision made observable: [`cyrup_acp::SessionManager::handle_load`] does its
/// work inside `cx.spawn` (`sessions.rs:2192-2232`) precisely so a long replay cannot block the
/// dispatch loop. Run straight-line from the handler, every later inbound message — including a
/// cancel — would wait behind the whole transcript.
///
/// # The witness is the probe, not the stop reason
///
/// `Turn::settle` maps BOTH `TurnOutcome::Cancelled` and `TurnOutcome::Replaced` to
/// `StopReason::Cancelled` (`turn.rs:353-354`, `:399-401`), so a prompt that settled `cancelled`
/// proves nothing about whether the cancel was ever seen — a load evicting the live session
/// produces the same answer. `authenticate` is answered INLINE in the dispatch loop
/// (`connection.rs:320-332`) while `session/load` returns immediately after `cx.spawn`, and inbound
/// dispatch is FIFO. So a probe sent AFTER the cancel, whose response beats the load's response
/// onto the wire, proves the loop dispatched both while the load was in flight. That is program
/// order, not a timing race.
///
/// # Where the interleaving actually happens
///
/// The replay loop itself is synchronous — `for update in replay { out.send_notification(..) }`
/// with no `await` (`sessions.rs:2205-2213`), because `send_notification` enqueues and returns — so
/// the window an inbound message lands in is `prepare_load`'s awaits: `load_target`, then
/// `gate.rebuild(build_and_install)` over a real session build. Do not read this test as proving
/// the burst is interruptible; it is not, and does not need to be.
#[test]
fn a_cancel_during_a_long_replay_is_observed_before_the_load_response() {
    /// Large enough that the replay dominates the load's wall time, small enough to keep the suite
    /// fast. The ordering assertion is structural rather than timed, so raising it buys nothing.
    const PAIRS: usize = 500;

    in_local_set(|| async {
        let mut h = Harness::start(vec![faux_assistant_message(
            vec![faux_text("unused")],
            StopReason::Stop,
        )])
        .await;
        let seeded = "01a07000-0000-7000-8000-00000000d0ad";
        let path = seed_session(&h.root(), h.cwd.path(), seeded, PAIRS);
        assert!(path.exists());

        let init = h
            .client
            .request(
                "initialize",
                json!({"protocolVersion": 1, "clientCapabilities": {}}),
            )
            .await;
        h.client.response_to(init).await;

        // Three frames in one burst, in this order, with nothing awaited between them.
        let load = h
            .client
            .request(
                "session/load",
                json!({"sessionId": seeded, "cwd": h.cwd.path().to_string_lossy(),
                       "mcpServers": []}),
            )
            .await;
        h.client
            .notify("session/cancel", json!({"sessionId": seeded}))
            .await;
        let probe = h
            .client
            .request("authenticate", json!({"methodId": "terminal"}))
            .await;

        let mut frames = h.client.drain_until(|v| is_response_to(v, load)).await;

        let load_at = index_of_response(&frames, load);
        let probe_at = index_of_response(&frames, probe);
        assert!(
            probe_at < load_at,
            "ACP-217/ACP-Q35: the dispatch loop was blocked by the load — a message sent AFTER the \
             cancel was answered at {probe_at}, behind the load response at {load_at}"
        );
        assert!(
            frames[probe_at].get("error").is_none(),
            "ACP-014: authenticate must never error: {}",
            frames[probe_at]
        );
        assert!(
            frames[load_at].get("error").is_none(),
            "ACP-123: a cancel during a load must not fail it: {}",
            frames[load_at]
        );

        // The replay really was long, and all of it preceded the response (`ACP-217`'s first half,
        // asserted here in-process as well as in `cyrup-it`).
        let users = updates_of(&frames[..load_at], "user_message_chunk").len();
        let agents = updates_of(&frames[..load_at], "agent_message_chunk").len();
        assert_eq!(
            (users, agents),
            (PAIRS, PAIRS),
            "the whole transcript must be replayed before the response"
        );
        assert!(
            frames[load_at..]
                .iter()
                .all(|v| v["params"]["update"]["sessionUpdate"] != "user_message_chunk"),
            "a replay chunk arrived after the response"
        );

        // The advertisement still follows the response.
        frames.extend(
            h.client
                .drain_until(|v| {
                    v["params"]["update"]["sessionUpdate"] == "available_commands_update"
                })
                .await,
        );
        assert!(
            index_of_response(&frames, load) < index_of_update(&frames, "available_commands_update"),
            "ACP-217: the command advertisement follows the response"
        );
    });
}

/// **ACP-005** — the transport ending disposes the live session.
///
/// Stdin EOF is the ACP host's NORMAL termination — the editor quit, or the user closed the project
/// window — and it arrives as `connect_to` returning. [`cyrup_acp::serve`] therefore calls
/// `SessionManager::shutdown` on EVERY exit path before propagating its result
/// (`connection.rs:447-465`), and `shutdown` ends in `live.runtime.dispose().await`
/// (`sessions.rs:1714-1722`).
///
/// The existing `shutdown_stops_the_turn_actor_and_the_config_pump` covers the slot take and the
/// binding teardown, and says in its own doc that the dispose half needs a real
/// `AgentSessionRuntime`. This is that half. Without it the regression is silent: no
/// `session_shutdown{reason:"quit"}` reaches any extension and `session_cancel` never fires, so
/// every tracked detached bash process group outlives the agent — one orphaned `setsid` group per
/// still-running background command, per editor quit, and nothing errors.
#[test]
fn the_transport_ending_disposes_the_live_session() {
    in_local_set(|| async {
        let mut h = Harness::start(vec![faux_assistant_message(
            vec![faux_text("hi")],
            StopReason::Stop,
        )])
        .await;
        let _session = h.open_session().await;
        assert_eq!(h.host.builds(), 1);
        assert!(
            h.host.shutdowns().is_empty(),
            "nothing has been disposed yet: {:?}",
            h.host.shutdowns()
        );

        // The editor quits.
        h.client.hang_up();
        h.served
            .await
            .expect("serve must return cleanly on a client hang-up");

        assert!(
            wait_for(|| !h.host.shutdowns().is_empty()).await,
            "ACP-005: the transport ended and the live session was never disposed — no \
             session_shutdown was emitted"
        );
        assert_eq!(
            h.host.shutdowns(),
            vec![(0, "quit".to_string())],
            "ACP-005: exactly one dispose, of the live runtime, with pi-acp's `quit` reason"
        );
    });
}
