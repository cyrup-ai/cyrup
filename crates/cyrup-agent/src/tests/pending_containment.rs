//! End-to-end containment for Pi's `"pending"` stop-reason sentinel (PROV-010 / AGENT-014 /
//! DRIFT-012, the `pending` half).
//!
//! `cyrup_core::StopReason::Pending` exists so cyrup's in-flight `partial` matches Pi's byte-for-
//! byte — Pi seeds `output.stopReason = "pending"` and attaches that same object to every
//! non-terminal event (`ai/src/api/anthropic-messages.ts:509`, `agent/src/proxy.ts:121-137`), and
//! `agent-loop.ts:314-341` forwards it verbatim as the `message_start` / `message_update` payload.
//! Adding a variant to a serialized enum is not free, so this suite pins the CONTAINMENT half of
//! the contract, which is what makes the addition safe:
//!
//! 1. `message_start` / `message_update` DO carry `"pending"` (the parity win);
//! 2. `message_end` / `turn_end` / `agent_end` and the settled transcript NEVER do — Pi enforces
//!    this with a `throw` whose catch sets `output.stopReason = "error"`
//!    (`anthropic-messages.ts:751-768`), cyrup with `StreamEvent::{end_of_stream,terminal}`;
//! 3. a truncated turn is therefore reported as an ERROR, not as a clean `stop` — the original
//!    defect: a stream cut off mid-turn was transcribed as a completed turn and fed back to the
//!    model as a finished assistant message.
//!
//! Offline throughout: the faux provider only. No network, no provider API.

use std::sync::Arc;

use crate::{Agent, AgentEvent, AgentMessage};
use cyrup_core::{AssistantMessage, StopReason};
use cyrup_provider::faux::{faux_assistant_message, faux_text};

use super::support::*;

fn assistant(m: &AgentMessage) -> Option<&AssistantMessage> {
    match m {
        AgentMessage::Assistant(a) => Some(a),
        _ => None,
    }
}

/// Run one prompt against a scripted faux response and return the recorded event stream.
async fn run(scripted: AssistantMessage) -> Vec<AgentEvent> {
    let agent = Agent::builder(model_ref(), faux_stream_fn(vec![scripted]).1).build();
    let recorder = Arc::new(EventRecorder::default());
    agent.subscribe(recorder.clone());
    let handle = agent.prompt("hi").await.unwrap();
    handle.finished().await;
    agent.wait_for_idle().await;
    recorder.snapshot()
}

/// The parity win: the in-flight assistant message the agent loop publishes carries Pi's sentinel,
/// on the wire, spelled `"pending"`.
///
/// These are the events cyrup serializes across real boundaries — to WASM extension guests
/// (`cyrup-ext/src/event.rs`, `HostEvent::MessageStart { message: to_value(message) }`) and to the
/// session-service fan-out. Before this change every one of them claimed `"stopReason":"stop"`
/// while the model was still mid-sentence.
#[tokio::test]
async fn message_start_and_update_publish_pi_pending() {
    let events = run(faux_assistant_message(
        vec![faux_text("hello there friend")],
        StopReason::Stop,
    ))
    .await;

    let mut saw_start = false;
    let mut saw_update = false;
    for e in &events {
        let (label, msg) = match e {
            AgentEvent::MessageStart { message } => ("message_start", message),
            AgentEvent::MessageUpdate { message, .. } => ("message_update", message),
            _ => continue,
        };
        let Some(a) = assistant(msg) else { continue };
        assert_eq!(
            a.stop_reason,
            StopReason::Pending,
            "{label} published a settled stop reason for an in-flight message"
        );
        assert_eq!(
            serde_json::to_value(a).unwrap()["stopReason"],
            "pending",
            "{label} wire spelling must be Pi's"
        );
        saw_start |= label == "message_start";
        saw_update |= label == "message_update";
    }
    assert!(saw_start, "no assistant message_start recorded");
    assert!(saw_update, "no message_update recorded");
}

/// The containment half: `pending` must not survive to any SETTLED event. `message_end` is the
/// event the session service persists on (`cyrup-session-svc/src/subscriber.rs`), so this is the
/// assertion that keeps the sentinel out of a session JSONL.
#[tokio::test]
async fn pending_never_reaches_a_settled_event_or_the_transcript() {
    for scripted_reason in [StopReason::Stop, StopReason::ToolUse, StopReason::Length] {
        let events = run(faux_assistant_message(
            vec![faux_text("hello there friend")],
            scripted_reason,
        ))
        .await;

        let mut settled = 0usize;
        for e in &events {
            let msgs: Vec<&AgentMessage> = match e {
                AgentEvent::MessageEnd { message } | AgentEvent::TurnEnd { message, .. } => {
                    vec![message]
                }
                AgentEvent::AgentEnd { messages } => messages.iter().map(|m| m.as_ref()).collect(),
                _ => continue,
            };
            for m in msgs {
                if let Some(a) = assistant(m) {
                    settled += 1;
                    assert_ne!(
                        a.stop_reason,
                        StopReason::Pending,
                        "{scripted_reason:?}: a settled event published the in-flight sentinel — \
                         this is what would land in the session JSONL"
                    );
                }
            }
        }
        assert!(settled > 0, "{scripted_reason:?}: no settled assistant events recorded");
    }
}

/// THE defect, end to end: a turn whose stream never delivered a stop reason must settle as an
/// ERROR carrying the diagnostic, not as a clean `stop`. Pi throws `"Faux response ended without a
/// stop reason"` (`ai/src/providers/faux.ts:393-395`) and its catch re-emits
/// `{type:"error", reason:"error"}` with the accumulated content intact.
#[tokio::test]
async fn a_turn_that_never_settled_is_an_error_not_a_completed_turn() {
    let events = run(faux_assistant_message(
        vec![faux_text("hello there friend")],
        StopReason::Pending,
    ))
    .await;

    let end = events
        .iter()
        .rev()
        .find_map(|e| match e {
            AgentEvent::MessageEnd { message } => assistant(message),
            _ => None,
        })
        .expect("an assistant message_end");

    assert_eq!(
        end.stop_reason,
        StopReason::Error,
        "a truncated turn was transcribed as {:?}",
        end.stop_reason
    );
    assert_eq!(
        end.error_message.as_deref(),
        Some("Faux response ended without a stop reason"),
        "the settled message must carry the diagnostic, not settle silently"
    );
    // Pi's catch re-emits `output` with its accumulated blocks, so the caller can see WHAT was cut
    // off rather than getting an empty message.
    let text: String = end
        .content
        .iter()
        .filter_map(|c| match c {
            cyrup_core::Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "hello there friend", "accumulated content must survive");

    // And the loop must have STOPPED on it: Pi returns immediately from `streamAssistantResponse`
    // for an error/aborted turn (agent-loop.ts:342-355). A `Pending` that slipped through the
    // `matches!(Error | Aborted)` guard would have continued into tool execution instead.
    assert!(
        matches!(events.last(), Some(AgentEvent::AgentEnd { .. })),
        "expected agent_end, got {:?}",
        events.last()
    );
}
