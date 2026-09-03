//! The session-side boundaries the core-loop type-driven refactor introduced (CLTR_2, CLTR_3B):
//! the strict `QueueMode` parser wrapped in pi's lenient settings fallback, and the resume bridge
//! that reconstitutes a persisted `bashExecution` custom entry as the `App` message a live
//! execution produces.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use crate::builder::parse_queue_mode;
use crate::event::raw_message_to_agent;
use cyrup_agent::{AgentMessage, AppRole, QueueMode};
use cyrup_session::agent_message::{AgentMessage as Raw, CustomRoleMessage};
use serde_json::json;

/// The settings boundary keeps pi's leniency (`getSteeringMode()` is `this.settings.steeringMode
/// || "one-at-a-time"`, unvalidated): the two pi strings parse, anything else falls back to
/// one-at-a-time instead of failing the session build.
#[test]
fn parse_queue_mode_is_lenient_over_the_strict_parser() {
    assert_eq!(parse_queue_mode("all"), QueueMode::All);
    assert_eq!(parse_queue_mode("one-at-a-time"), QueueMode::OneAtATime);
    for bad in ["ALL", "", "steer", "one_at_a_time"] {
        assert_eq!(parse_queue_mode(bad), QueueMode::OneAtATime, "{bad:?} falls back as pi does");
    }
}

fn raw_custom(custom_type: &str, content: serde_json::Value) -> Raw {
    Raw::Custom(CustomRoleMessage {
        custom_type: custom_type.to_string(),
        content,
        display: true,
        details: None,
        timestamp: 1,
    })
}

/// A persisted `!` execution resumes as `Custom { custom_type: "bashExecution" }`; the bridge
/// reconstitutes the `App` message a live execution produces — payload preserved, `role` stamped
/// with the enum's tag — so the model never reads a bash result as a custom entry.
#[test]
fn resume_bridge_reconstitutes_bash_execution_as_app() {
    let raw = raw_custom("bashExecution", json!({ "command": "ls", "output": "a\nb" }));
    match raw_message_to_agent(&raw) {
        AgentMessage::App { role, payload } => {
            assert_eq!(role, AppRole::BashExecution);
            assert_eq!(payload.get("command"), Some(&json!("ls")));
            assert_eq!(payload.get("output"), Some(&json!("a\nb")));
            assert_eq!(payload.get("role"), Some(&json!("bashExecution")));
        }
        other => panic!("expected App, got {other:?}"),
    }
}

/// Everything that is NOT one of the three app roles stays `Custom` — the variant reserved for
/// extension messages — including a role-looking tag with a non-object payload.
#[test]
fn resume_bridge_leaves_extension_messages_custom() {
    let memo = raw_custom("memo", json!({ "note": "keep" }));
    assert!(matches!(raw_message_to_agent(&memo), AgentMessage::Custom { kind, .. } if kind == "memo"));
    let non_object = raw_custom("bashExecution", json!("not an object"));
    assert!(matches!(
        raw_message_to_agent(&non_object),
        AgentMessage::Custom { kind, .. } if kind == "bashExecution"
    ));
}
