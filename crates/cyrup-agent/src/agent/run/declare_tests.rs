#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use super::*;
use cyrup_core::{Content, ToolReference};
use std::collections::BTreeSet;

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

/// AGENT-039 — the request projection filters ONLY hidden declarations and keeps everything else,
/// which is pi's `_installHiddenDeclarationsProjection`. The assertion here previously pinned the
/// opposite (a blanket strip, with a declaration-only message dropped); that was a cyrup-original
/// divergence, and it is what made the native mid-conversation tool-change shapes unreachable.
#[test]
fn the_request_projection_removes_only_hidden_declarations() {
    let hidden = BTreeSet::from(["a".to_string(), "c".to_string()]);
    let out = project_hidden_declarations(
        vec![
            Message::System(SystemMessage {
                tools_added: vec![tool("a")],
                timestamp: 1,
                ..SystemMessage::default()
            }),
            Message::System(SystemMessage {
                content: vec![Content::text("keep me")],
                tools_added: vec![tool("b")],
                tools_removed: vec![ToolReference::new("c"), ToolReference::new("d")],
                timestamp: 2,
                ..SystemMessage::default()
            }),
        ],
        &hidden,
    );

    // Both messages survive: pi keeps a message whose declarations were all hidden, empty.
    assert_eq!(out.len(), 2, "{out:?}");
    match &out[0] {
        Message::System(s) => {
            assert!(s.tools_added.is_empty(), "the hidden `a` is projected out");
            assert_eq!(s.timestamp, 1, "every other field is untouched");
        }
        other => panic!("{other:?}"),
    }
    match &out[1] {
        Message::System(s) => {
            assert_eq!(s.content, vec![Content::text("keep me")]);
            assert_eq!(
                s.tools_added
                    .iter()
                    .map(|t| t.name.as_str())
                    .collect::<Vec<_>>(),
                ["b"],
                "a non-hidden declaration STAYS in the transcript"
            );
            assert_eq!(
                s.tools_removed
                    .iter()
                    .map(|t| t.name.as_str())
                    .collect::<Vec<_>>(),
                ["d"],
                "the hidden removal is projected out, the visible one kept"
            );
        }
        other => panic!("{other:?}"),
    }
}

/// With nothing hidden the transcript is returned untouched — the common case, and the one the
/// blanket strip used to mangle.
#[test]
fn an_empty_hidden_set_keeps_every_declaration() {
    let messages = vec![Message::System(SystemMessage {
        content: vec![Content::text("prompt")],
        tools_added: vec![tool("a"), tool("b")],
        tools_removed: vec![ToolReference::new("c")],
        timestamp: 7,
        ..SystemMessage::default()
    })];
    let out = project_hidden_declarations(messages.clone(), &BTreeSet::new());
    assert_eq!(out, messages);
}
