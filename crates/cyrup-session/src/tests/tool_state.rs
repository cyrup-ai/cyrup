//! CODE-005 — the loadout across a compaction.
//!
//! A compaction drops the entries before the first kept one, and pi's `buildContextEntries` drops
//! every SYSTEM message from the kept range as well (`session-manager.ts:506` @v1.0.1), so the
//! system messages that declared the loadout would be gone. The compaction entry carries the
//! replayed prompt and tool state itself (`CompactionEntry.systemMessage`, `:103`, written by
//! `appendCompaction` at `:1096-1116`), and its projection is that message followed by the summary
//! (`sessionEntryToContextMessages`, `:462-463`). These tests read the ON-DISK entry and the
//! projection a resumed session is seeded from.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::PathBuf;

use crate::agent_message::AgentMessage;
use crate::{Entry, KnownEntry, NewSessionOpts, SessionManager};
use cyrup_core::{Content, Message, SystemMessage, ToolDef, ToolReference};

fn tool(name: &str) -> ToolDef {
    ToolDef {
        name: name.to_string(),
        description: format!("{name} desc"),
        parameters: serde_json::json!({ "type": "object", "properties": {} }),
        constrained_sampling: None,
    }
}

fn declare(added: &[&str], removed: &[&str]) -> Message {
    Message::System(SystemMessage {
        tools_added: added.iter().map(|n| tool(n)).collect(),
        tools_removed: removed.iter().map(|n| ToolReference::new(*n)).collect(),
        timestamp: 5,
        ..SystemMessage::default()
    })
}

fn user(text: &str) -> Message {
    Message::User {
        content: vec![Content::text(text)],
        timestamp: 1,
    }
}

fn manager() -> SessionManager {
    SessionManager::in_memory(
        &PathBuf::from("/proj/tool_state"),
        NewSessionOpts::default(),
    )
    .unwrap()
}

/// The names of the tools a projection declares, replayed the way a provider replays them.
fn declared(messages: &[AgentMessage]) -> Vec<String> {
    let system: Vec<Message> = messages
        .iter()
        .filter_map(|m| match m {
            AgentMessage::Core(core @ Message::System(_)) => Some(core.clone()),
            _ => None,
        })
        .collect();
    cyrup_provider::get_current_tools(&system)
        .into_iter()
        .map(|t| t.name)
        .collect()
}

fn compaction_system_message(m: &SessionManager) -> Option<SystemMessage> {
    m.entries().iter().find_map(|e| match e {
        Entry::Known(KnownEntry::Compaction { system_message, .. }) => system_message.clone(),
        _ => None,
    })
}

/// `appendCompaction` records the state replayed from the branch at that moment: both declarations
/// and the removal between them are folded into the one message, stamped with the entry's own time.
#[test]
fn a_compaction_records_the_replayed_loadout() {
    let mut m = manager();
    m.append_message(declare(&["a", "b"], &[])).unwrap();
    let u1 = m.append_message(user("one")).unwrap();
    m.append_message(declare(&["c"], &["b"])).unwrap();
    m.append_message(user("two")).unwrap();
    m.append_compaction("SUMMARY".into(), u1, 100, None, None, false)
        .unwrap();

    let recorded = compaction_system_message(&m).expect("the compaction carries the loadout");
    let names: Vec<&str> = recorded
        .tools_added
        .iter()
        .map(|t| t.name.as_str())
        .collect();
    assert_eq!(names, ["a", "c"], "the replay, with `b` removed");
    assert!(
        recorded.tools_removed.is_empty(),
        "a replay has no removals"
    );
    assert_ne!(recorded.timestamp, 5, "stamped with the entry's time");
}

/// The on-disk key is pi's `systemMessage`, and it is absent when nothing was declared.
#[test]
fn the_compaction_entry_names_the_field_as_pi_does() {
    let mut m = manager();
    m.append_message(declare(&["a"], &[])).unwrap();
    let u1 = m.append_message(user("one")).unwrap();
    m.append_message(user("two")).unwrap();
    let with = m
        .append_compaction("S".into(), u1, 1, None, None, false)
        .unwrap();
    let line = serde_json::to_string(m.entry(&with).unwrap()).unwrap();
    assert!(
        line.contains(",\"systemMessage\":{\"role\":\"system\",\"content\":\"\",\"toolsAdded\":["),
        "{line}"
    );
    // A snapshot is a replay, so `timestamp` is the LAST key (pi: `{ ...getCurrentSystemMessage(…),
    // timestamp }`, `session-manager.ts:1283` — the spread leaves `timestamp` where the snapshot put
    // it), unlike the system row of a `message` entry where `timestamp` precedes `toolsAdded`.
    let snapshot = &line[line.find("\"systemMessage\":").unwrap()..];
    assert!(
        snapshot.find("\"toolsAdded\"").unwrap() < snapshot.find("\"timestamp\"").unwrap(),
        "{line}"
    );

    let mut bare = manager();
    let u1 = bare.append_message(user("one")).unwrap();
    bare.append_message(user("two")).unwrap();
    let without = bare
        .append_compaction("S".into(), u1, 1, None, None, false)
        .unwrap();
    let line = serde_json::to_string(bare.entry(&without).unwrap()).unwrap();
    assert!(!line.contains("systemMessage"), "{line}");
}

/// The projection a resumed session is seeded from: the compaction's `systemMessage` first, then
/// its summary, and no system message of the kept range — whose state the first one already holds.
#[test]
fn the_projection_after_a_compaction_leads_with_the_recorded_loadout() {
    let mut m = manager();
    m.append_message(declare(&["a", "b"], &[])).unwrap();
    m.append_message(user("one")).unwrap();
    // A system message INSIDE the kept range: it must not be projected twice.
    let kept = m.append_message(declare(&["c"], &[])).unwrap();
    m.append_message(user("two")).unwrap();
    m.append_compaction("SUMMARY".into(), kept, 100, None, None, false)
        .unwrap();
    m.append_message(user("three")).unwrap();

    let raw = m.build_context_raw();
    let shape: Vec<&str> = raw
        .iter()
        .map(|m| match m {
            AgentMessage::Core(Message::System(_)) => "system",
            AgentMessage::Core(Message::User { .. }) => "user",
            AgentMessage::CompactionSummary(_) => "summary",
            _ => "other",
        })
        .collect();
    assert_eq!(
        shape,
        ["system", "summary", "user", "user"],
        "the kept range's system message is dropped; its state is the first message's"
    );
    assert_eq!(declared(&raw), ["a", "b", "c"]);
    let llm = m.build_context().messages;
    assert!(
        matches!(llm.first(), Some(Message::System(_))),
        "the model's context leads with it too: {llm:?}"
    );
}

/// A session written before the transcript carried system messages has no `systemMessage` on its
/// compactions, and projects exactly as it did.
#[test]
fn a_compaction_without_a_recorded_loadout_projects_the_summary_alone() {
    let mut m = manager();
    let u1 = m.append_message(user("one")).unwrap();
    m.append_message(user("two")).unwrap();
    m.append_compaction("SUMMARY".into(), u1, 100, None, None, false)
        .unwrap();
    let raw = m.build_context_raw();
    assert!(
        matches!(raw.first(), Some(AgentMessage::CompactionSummary(_))),
        "{raw:?}"
    );
    assert!(declared(&raw).is_empty());
}

/// pi `getMessagesFromProjectedEntryForCompaction` (`compaction.ts:98-102` @v1.0.1): a system
/// message is prompt state, not conversation, so it is never summarized. A session whose only
/// entry before the first user message is the loadout declaration has NOTHING to compact when the
/// budget keeps every turn — the declaration is not "history".
#[test]
fn a_system_message_is_never_summarized() {
    use crate::compaction::prepare_compaction;
    use crate::compaction::settings::CompactionSettings;
    use crate::compaction::tokens::TokenCache;

    let mut m = manager();
    m.append_message(declare(&["a"], &[])).unwrap();
    m.append_message(user("one")).unwrap();
    m.append_message(Message::Assistant(cyrup_core::AssistantMessage {
        content: vec![Content::text("answer")],
        provider: "faux".into(),
        model: "faux-1".into(),
        api: "faux".into(),
        response_model: None,
        response_id: None,
        provider_thinking_level: None,
        diagnostics: None,
        usage: cyrup_core::Usage::default(),
        stop_reason: cyrup_core::StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 0,
    }))
    .unwrap();
    let path: Vec<Entry> = m.branch_path(None).into_iter().cloned().collect();
    let settings = CompactionSettings {
        enabled: true,
        reserve_tokens: 0,
        keep_recent_tokens: 1_000_000,
    };
    assert!(
        prepare_compaction(&path, &TokenCache::default(), &settings).is_none(),
        "the declaration alone is not history to summarize"
    );
}
