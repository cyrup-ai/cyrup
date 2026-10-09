//! EXT-078 — the session half of pi's boundary drafts: `appendContextEdit` with its refusals
//! (`core/session-manager.ts:1360-1395` @v1.1.0), the self-retaining compaction
//! (`firstKeptEntryId: null`, `:1261-1286`), the drafts' JSON, and the in-memory preview they are
//! tried on first (`_createBoundaryPreviewManager`, `core/agent-session.ts:987-993`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::PathBuf;

use crate::agent_message::AgentMessage;
use crate::entry::{ContextEditReplacement, ContextEditableContent};
use crate::{Entry, KnownEntry, NewSessionOpts, SessionBoundaryDraft, SessionManager};
use cyrup_core::{AssistantMessage, Content, Message, StopReason};
use serde_json::json;

fn manager() -> SessionManager {
    SessionManager::in_memory(&PathBuf::from("/proj/boundary"), NewSessionOpts::default()).unwrap()
}

fn user(text: &str) -> Message {
    Message::User {
        content: vec![Content::text(text)],
        timestamp: 1,
    }
}

fn assistant(text: &str) -> Message {
    Message::Assistant(AssistantMessage {
        content: vec![Content::text(text)],
        provider: "faux".into(),
        model: "faux-1".into(),
        api: "faux".into(),
        response_model: None,
        response_id: None,
        provider_thinking_level: None,
        thinking_level: None,
        diagnostics: None,
        usage: cyrup_core::Usage::default(),
        stop_reason: StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 0,
        duration_ms: None,
    })
}

fn texts(messages: &[AgentMessage]) -> Vec<String> {
    messages
        .iter()
        .map(|m| match m {
            AgentMessage::Core(Message::User { content, .. }) => format!("user:{}", text(content)),
            AgentMessage::Core(Message::Assistant(a)) => format!("assistant:{}", text(&a.content)),
            AgentMessage::CompactionSummary(c) => format!("summary:{}", c.summary),
            AgentMessage::Custom(c) => format!("custom:{}", c.content),
            other => format!("{other:?}"),
        })
        .collect()
}

fn text(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.to_string()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_context_edit_is_refused_with_pis_text() {
    let mut m = manager();
    let first = m.append_message(user("one")).unwrap();
    let custom = m.append_custom_entry("state", None).unwrap();
    let missing = cyrup_core::EntryId::from("nope");
    assert_eq!(
        m.append_context_edit(&missing, None)
            .unwrap_err()
            .to_string(),
        "Entry nope not found"
    );
    assert_eq!(
        m.append_context_edit(&custom, None)
            .unwrap_err()
            .to_string(),
        format!("Entry {custom} does not contribute editable model content")
    );
    // Off the active branch: branch back to `first` and grow a sibling.
    let off = m.append_message(assistant("old")).unwrap();
    m.branch(&first).unwrap();
    m.append_message(assistant("new")).unwrap();
    assert_eq!(
        m.append_context_edit(&off, None).unwrap_err().to_string(),
        format!("Entry {off} is not on the active branch")
    );
}

#[test]
fn a_context_edit_omits_or_replaces_and_normalizes_assistant_text() {
    let mut m = manager();
    let u = m.append_message(user("hello")).unwrap();
    let a = m.append_message(assistant("draft")).unwrap();
    m.append_context_edit(
        &a,
        Some(ContextEditReplacement {
            content: ContextEditableContent::Text("final".into()),
        }),
    )
    .unwrap();
    m.append_context_edit(&u, None).unwrap();
    let Some(Entry::Known(KnownEntry::ContextEdit { replacement, .. })) =
        m.entries().iter().rev().nth(1)
    else {
        panic!("the assistant edit");
    };
    assert_eq!(
        replacement.as_ref().unwrap().content,
        ContextEditableContent::Blocks(vec![Content::text("final")]),
        "an assistant's string replacement is stored as one text block"
    );
    assert_eq!(texts(&m.build_context_raw()), ["assistant:final"]);
}

#[test]
fn a_compaction_without_a_first_kept_entry_keeps_nothing_before_it() {
    let mut m = manager();
    m.append_message(user("one")).unwrap();
    m.append_message(assistant("two")).unwrap();
    let id = m
        .append_compaction_keeping("SUMMARY".into(), None, 10, None, None, true)
        .unwrap();
    m.append_message(user("three")).unwrap();
    let Some(Entry::Known(KnownEntry::Compaction {
        first_kept_entry_id,
        ..
    })) = m.entry(&id)
    else {
        panic!("a compaction");
    };
    assert_eq!(
        first_kept_entry_id.as_ref(),
        Some(&id),
        "pi stores its own id"
    );
    assert_eq!(
        texts(&m.build_context_raw()),
        ["summary:SUMMARY", "user:three"]
    );
}

#[test]
fn drafts_parse_from_pis_json_and_apply_in_order_on_a_preview_only() {
    let mut m = manager();
    let u = m.append_message(user("one")).unwrap();
    let a = m.append_message(assistant("two")).unwrap();
    let drafts: Vec<SessionBoundaryDraft> = serde_json::from_value(json!([
        {"type": "custom", "customType": "mark", "data": {"n": 1}},
        {"type": "custom_message", "customType": "note", "content": "noted", "display": true},
        {"type": "context_edit", "targetId": a.to_string(), "replacement": null},
        {"type": "compaction", "summary": "S", "firstKeptEntryId": u.to_string()},
    ]))
    .unwrap();
    let before = m.entries().len();

    let mut preview = m.branch_preview();
    let appended = preview.apply_boundary_drafts(&drafts).unwrap();
    assert_eq!(appended.len(), 4);
    assert_eq!(m.entries().len(), before, "the preview writes nothing back");
    let kinds: Vec<&str> = appended
        .iter()
        .map(|e| match e {
            Entry::Known(KnownEntry::Custom { .. }) => "custom",
            Entry::Known(KnownEntry::CustomMessage { .. }) => "custom_message",
            Entry::Known(KnownEntry::ContextEdit { .. }) => "context_edit",
            Entry::Known(KnownEntry::Compaction { from_hook, .. }) => {
                assert_eq!(*from_hook, Some(true));
                "compaction"
            }
            _ => "other",
        })
        .collect();
    assert_eq!(
        kinds,
        ["custom", "custom_message", "context_edit", "compaction"]
    );
    assert_eq!(
        texts(&preview.build_context_raw()),
        ["summary:S", "user:one", "custom:\"noted\""],
        "the omitted assistant is gone and the custom message is context"
    );
    let projection = preview.session_projection();
    let omitted = projection
        .iter()
        .find(|(e, _)| e.id() == a)
        .expect("the omitted entry is still a projected entry");
    assert!(omitted.1.is_empty(), "contributing no messages");

    // A bad draft stops the walk with its error, the drafts before it applied.
    let bad: Vec<SessionBoundaryDraft> = serde_json::from_value(json!([
        {"type": "custom", "customType": "ok"},
        {"type": "context_edit", "targetId": "nope", "replacement": null},
    ]))
    .unwrap();
    let mut preview = m.branch_preview();
    assert_eq!(
        preview.apply_boundary_drafts(&bad).unwrap_err().to_string(),
        "Entry nope not found"
    );
}
