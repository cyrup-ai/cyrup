#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use super::*;
use cyrup_core::{Content, ToolReference};

fn tool(name: &str) -> ToolDef {
    ToolDef {
        name: name.to_string(),
        description: format!("{name} desc"),
        parameters: serde_json::json!({ "type": "object" }),
        constrained_sampling: None,
    }
}

fn user(text: &str) -> AgentMessage {
    AgentMessage::User {
        content: vec![Content::text(text)],
        timestamp: Some(1),
    }
}

fn declaring(names: &[&str]) -> Arc<AgentMessage> {
    Arc::new(AgentMessage::System(SystemMessage {
        tools_added: names.iter().map(|n| tool(n)).collect(),
        timestamp: 1,
        ..SystemMessage::default()
    }))
}

fn system_at(messages: &[AgentMessage], index: usize) -> &SystemMessage {
    match messages.get(index) {
        Some(AgentMessage::System(s)) => s,
        other => panic!("expected a system message at {index}, got {other:?}"),
    }
}

/// `agent-loop.ts:359-362`: with no pending system message, one is inserted before the first
/// non-system pending message.
#[test]
fn a_missing_declaration_is_inserted_before_the_first_prompt_message() {
    let out = declare_tool_changes(&[], &[tool("a")], vec![user("hi")], 7);
    assert_eq!(out.len(), 2);
    assert_eq!(system_at(&out, 0).tools_added, vec![tool("a")]);
    assert_eq!(system_at(&out, 0).timestamp, 7);
    assert!(matches!(out[1], AgentMessage::User { .. }));
}

/// `agent-loop.ts:359-362`, `insertIndex === -1`: nothing pending, so the update goes last.
#[test]
fn with_nothing_pending_the_declaration_is_the_only_message() {
    let out = declare_tool_changes(&[], &[tool("a")], Vec::new(), 7);
    assert_eq!(out.len(), 1);
    assert_eq!(system_at(&out, 0).tools_added, vec![tool("a")]);
}

/// `agent-loop.ts:351-363`: a transcript that already declares the executable set gets no row,
/// and the pending messages come back untouched.
#[test]
fn a_declared_loadout_leaves_the_pending_messages_untouched() {
    let committed = [declaring(&["a"])];
    let out = declare_tool_changes(&committed, &[tool("a")], vec![user("hi")], 7);
    assert_eq!(out, vec![user("hi")]);
}

/// `agent-loop.ts:353-357`: a pending system message's tool fields are INTENT — replaced by the
/// delta between the committed transcript and the executable set, so replay yields exactly the
/// executable set.
#[test]
fn a_pending_system_message_has_its_tool_intent_replaced() {
    let pending = vec![
        AgentMessage::System(SystemMessage {
            content: vec![Content::text("plan mode")],
            tools_added: vec![tool("ghost")],
            tools_removed: vec![ToolReference::new("phantom")],
            timestamp: 3,
            ..SystemMessage::default()
        }),
        user("hi"),
    ];
    let out = declare_tool_changes(&[], &[tool("a")], pending, 7);
    assert_eq!(out.len(), 2, "merged into the pending message, not added");
    let merged = system_at(&out, 0);
    assert_eq!(merged.tools_added, vec![tool("a")]);
    assert!(merged.tools_removed.is_empty());
    assert_eq!(merged.timestamp, 3, "the caller's message is kept");
    assert_eq!(merged.content, vec![Content::text("plan mode")]);
}

/// The same, when the pending message's intent was already right: the caller's message comes
/// back unchanged (`:354-355`).
#[test]
fn a_pending_system_message_that_declares_nothing_is_kept_as_is() {
    let committed = [declaring(&["a"])];
    let pending = vec![AgentMessage::System(SystemMessage {
        content: vec![Content::text("note")],
        timestamp: 3,
        ..SystemMessage::default()
    })];
    let out = declare_tool_changes(&committed, &[tool("a")], pending.clone(), 7);
    assert_eq!(out, pending);
}

/// A changed definition is a removal followed by an addition (`getToolStateChanges`).
#[test]
fn a_redefined_tool_is_removed_and_added() {
    let committed = [declaring(&["a"])];
    let mut changed = tool("a");
    changed.description = "now different".to_string();
    let out = declare_tool_changes(&committed, std::slice::from_ref(&changed), Vec::new(), 7);
    let row = system_at(&out, 0);
    assert_eq!(row.tools_added, vec![changed]);
    assert_eq!(row.tools_removed, vec![ToolReference::new("a")]);
}

/// The request projection removes the declarations and keeps every other part of a system
/// message; a message that held only declarations disappears.
#[test]
fn the_request_projection_strips_declarations_only() {
    let out = without_tool_declarations(vec![
        Message::System(SystemMessage {
            tools_added: vec![tool("a")],
            timestamp: 1,
            ..SystemMessage::default()
        }),
        Message::System(SystemMessage {
            content: vec![Content::text("keep me")],
            tools_added: vec![tool("b")],
            tools_removed: vec![ToolReference::new("c")],
            timestamp: 2,
            ..SystemMessage::default()
        }),
    ]);
    assert_eq!(out.len(), 1, "{out:?}");
    match &out[0] {
        Message::System(s) => {
            assert_eq!(s.content, vec![Content::text("keep me")]);
            assert!(s.tools_added.is_empty() && s.tools_removed.is_empty());
        }
        other => panic!("{other:?}"),
    }
}
