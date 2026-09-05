//! End-to-end: the real ACP connection, driven over an in-memory transport, against a session
//! whose provider is scripted.
//!
//! Every other test in this crate is a unit test over fixture events. This file is the one that
//! answers "does an editor actually get what the guide says it gets" — it builds a real
//! [`cyrup_acp::AgentSessionRuntime`] over `FauxProvider`, installs it behind the real
//! [`cyrup_acp::AcpHost`] trait, and runs [`cyrup_acp::serve`] over a `Lines` transport whose two
//! halves are `futures` channels the harness holds. Frames on those channels are the bytes a client
//! would see.
//!
//! No network, no credential, no spawned process, and deterministic: `serve` takes an arbitrary
//! transport for exactly this reason (see its doc in `connection.rs`), and the offline
//! `FauxProvider` is the same one `crates/cyrup-it/tests/bin/acp_session.rs` drives.
//!
//! The harness itself lives in `tests/support/mod.rs`, shared with `tests/wire_gaps.rs`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod support;

use cyrup_core::StopReason;
use cyrup_provider::faux::{faux_assistant_message, faux_text, faux_tool_call};
use serde_json::json;
use support::{Harness, in_local_set, is_response_to, updates_of};

/// A tool-calling turn reaches the client as an announce followed by updates, and the prompt
/// resolves once.
///
/// This is the path the guide promises ("each call shown as it runs") and the one no test
/// exercised end to end before: `translate.rs` proved the mapping over fixture events, but nothing
/// drove a real scripted turn through a real connection.
#[test]
fn a_tool_calling_turn_streams_to_the_client() {
    in_local_set(|| async {
        let mut h = Harness::start(vec![
            faux_assistant_message(
                vec![faux_tool_call(
                    "edit",
                    json!({"path": "hello.txt", "edits": [{"oldText": "two", "newText": "TWO"}]}),
                )],
                StopReason::ToolUse,
            ),
            faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
        ])
        .await;

        let session = h.open_session().await;

        let id = h
            .client
            .request(
                "session/prompt",
                json!({"sessionId": session, "prompt": [{"type": "text", "text": "uppercase the second line"}]}),
            )
            .await;
        let frames = h.client.drain_until(|v| is_response_to(v, id)).await;

        let announces = updates_of(&frames, "tool_call");
        assert!(
            !announces.is_empty(),
            "no tool_call announce reached the client; frames: {frames:#?}"
        );

        let response = frames
            .iter()
            .find(|v| is_response_to(v, id))
            .expect("the prompt must resolve");
        assert!(
            response.get("error").is_none(),
            "the prompt failed: {response}"
        );
        assert_eq!(
            response["result"]["stopReason"], "end_turn",
            "a scripted tool turn then a text turn must settle as end_turn: {response}"
        );

        // The tool must actually RUN. A `failed` here means the session had no such tool, which
        // is what an empty registry looks like from the wire.
        let statuses: Vec<&str> = updates_of(&frames, "tool_call_update")
            .iter()
            .filter_map(|v| v["params"]["update"]["status"].as_str())
            .collect();
        assert!(
            statuses.contains(&"completed"),
            "the edit never completed; statuses were {statuses:?} in {frames:#?}"
        );
        assert!(
            !statuses.contains(&"failed"),
            "the edit failed; statuses were {statuses:?} in {frames:#?}"
        );

        // The guide promises edits arrive as diffs. This is that promise.
        let has_diff = updates_of(&frames, "tool_call_update").iter().any(|v| {
            v["params"]["update"]["content"]
                .as_array()
                .is_some_and(|cs| cs.iter().any(|c| c["type"] == "diff"))
        });
        assert!(
            has_diff,
            "no diff content block reached the client: {frames:#?}"
        );

        // …and the edit is on disk.
        assert_eq!(
            std::fs::read_to_string(h.cwd.path().join("hello.txt")).unwrap(),
            "one\nTWO\nthree\n",
            "the tool call reported success without changing the file"
        );

        // The announce is always first for an id: a client that saw an update before the announce
        // would have nothing to attach it to.
        let first_tool_frame = frames.iter().find(|v| {
            v["params"]["update"]["sessionUpdate"]
                .as_str()
                .is_some_and(|k| k.starts_with("tool_call"))
        });
        assert_eq!(
            first_tool_frame.map(|v| v["params"]["update"]["sessionUpdate"].as_str()),
            Some(Some("tool_call")),
            "the first tool frame for a call must be the announce, not an update"
        );
    });
}

/// `session/list` and `session/delete` operate on the real session tree.
#[test]
fn sessions_are_listed_and_deleted_through_the_wire() {
    in_local_set(|| async {
        let mut h = Harness::start(vec![faux_assistant_message(
            vec![faux_text("hi")],
            StopReason::Stop,
        )])
        .await;
        let session = h.open_session().await;

        let id = h.client.request("session/list", json!({})).await;
        let listed = h.client.response_to(id).await;
        assert!(
            listed.get("error").is_none(),
            "session/list failed: {listed}"
        );

        let id = h
            .client
            .request("session/delete", json!({"sessionId": session}))
            .await;
        let deleted = h.client.response_to(id).await;
        assert!(
            deleted.get("error").is_none(),
            "session/delete of the live session failed: {deleted}"
        );
    });
}

/// The exact ordered sequence a client sees for a tool-calling turn.
///
/// The other tests assert properties; this one pins the shape, so a regression that reorders or
/// drops a frame is named rather than merely failing somewhere downstream.
#[test]
fn the_frame_sequence_is_stable() {
    in_local_set(|| async {
        let mut h = Harness::start(vec![
            faux_assistant_message(
                vec![faux_tool_call("edit", json!({"path": "hello.txt", "edits": [{"oldText": "two", "newText": "TWO"}]}))],
                StopReason::ToolUse,
            ),
            faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
        ]).await;
        let session = h.open_session().await;
        let id = h
            .client
            .request("session/prompt", json!({"sessionId": session, "prompt": [{"type":"text","text":"uppercase the second line"}]}))
            .await;
        let frames = h.client.drain_until(|v| is_response_to(v, id)).await;

        let kinds: Vec<String> = frames
            .iter()
            .map(|f| {
                f["params"]["update"]["sessionUpdate"]
                    .as_str()
                    .map_or_else(|| "<response>".to_string(), |k| {
                        f["params"]["update"]["status"]
                            .as_str()
                            .map_or_else(|| k.to_string(), |st| format!("{k}:{st}"))
                    })
            })
            .collect();

        assert_eq!(
            kinds,
            vec![
                "available_commands_update",
                "session_info_update",
                "tool_call",
                "tool_call_update:pending",
                "tool_call_update:in_progress",
                "tool_call_update:completed",
                "agent_message_chunk",
                // The context-window meter. Absent until `AcpTurnAgent` forwarded
                // `TurnAgent::usage` — the trait's default is `None`, so the production wrapper
                // inheriting it shipped a meter that never filled while every unit test of the
                // emission passed. This entry is what says the wrapper still forwards.
                "usage_update",
                "session_info_update",
                "<response>",
            ],
            "the client-visible frame sequence changed: {frames:#?}"
        );
    });
}

/// A bash tool call reaches the client as a terminal, and its output arrives.
#[test]
fn a_bash_call_streams_terminal_output() {
    in_local_set(|| async {
        let mut h = Harness::start(vec![
            faux_assistant_message(
                vec![faux_tool_call("bash", json!({"command": "echo cyrup-acp-e2e"}))],
                StopReason::ToolUse,
            ),
            faux_assistant_message(vec![faux_text("ok")], StopReason::Stop),
        ]).await;
        let session = h.open_session().await;
        let id = h
            .client
            .request("session/prompt", json!({"sessionId": session, "prompt": [{"type":"text","text":"echo"}]}))
            .await;
        let frames = h.client.drain_until(|v| is_response_to(v, id)).await;

        let statuses: Vec<&str> = updates_of(&frames, "tool_call_update")
            .iter()
            .filter_map(|v| v["params"]["update"]["status"].as_str())
            .collect();
        assert!(
            statuses.contains(&"completed"),
            "bash never completed; statuses {statuses:?} in {frames:#?}"
        );

        let blob = serde_json::to_string(&frames).unwrap_or_default();
        assert!(
            blob.contains("cyrup-acp-e2e"),
            "the command output never reached the client: {frames:#?}"
        );
    });
}

/// A `session/set_mode` that changes nothing still tells the client where the mode landed.
///
/// `ACP-072`. The config pump emits `current_mode_update` on `ThinkingLevelChanged`, but a request
/// whose applied level equals the current one raises no such event, so the pump stays silent. A
/// client that optimistically moved its selector would be wrong for the rest of the session. The
/// echo is sent by `set_mode` itself for exactly that case.
#[test]
fn a_no_op_set_mode_still_echoes_the_applied_level() {
    in_local_set(|| async {
        let mut h = Harness::start(vec![faux_assistant_message(
            vec![faux_text("hi")],
            StopReason::Stop,
        )])
        .await;

        let id = h
            .client
            .request(
                "initialize",
                json!({"protocolVersion": 1, "clientCapabilities": {}}),
            )
            .await;
        h.client.response_to(id).await;
        let id = h
            .client
            .request(
                "session/new",
                json!({"cwd": h.cwd.path().to_string_lossy(), "mcpServers": []}),
            )
            .await;
        let opened = h.client.response_to(id).await;
        let session = opened["result"]["sessionId"].as_str().unwrap().to_string();
        let current = opened["result"]["modes"]["currentModeId"]
            .as_str()
            .expect("session/new advertises a current mode")
            .to_string();

        // Set it to what it already is: nothing changes, so the pump will not speak.
        let id = h
            .client
            .request(
                "session/set_mode",
                json!({"sessionId": session, "modeId": current}),
            )
            .await;
        let frames = h.client.drain_until(|v| is_response_to(v, id)).await;

        let modes = updates_of(&frames, "current_mode_update");
        assert!(
            !modes.is_empty(),
            "a no-op set_mode said nothing, so a client that moved its selector stays wrong: {frames:#?}"
        );
        assert_eq!(
            modes[0]["params"]["update"]["currentModeId"].as_str(),
            Some(current.as_str()),
            "the echo must carry the APPLIED level, not the requested one"
        );
        assert!(
            !updates_of(&frames, "config_option_update").is_empty(),
            "the echo must also re-derive the option set, for a client rendering the dropdown"
        );
    });
}
