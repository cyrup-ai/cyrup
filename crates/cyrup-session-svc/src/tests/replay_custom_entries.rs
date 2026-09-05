//! EXT-041 — a `custom` ENTRY has to survive the replay projection.
//!
//! pi's replay walk is `renderSessionEntries(this.sessionManager.buildContextEntries())`
//! (`modes/interactive/interactive-mode.ts:3910` @v0.84.4) and its flat-map passes a `custom` entry
//! through UNPROJECTED — `if (entry.type === "custom") return [entry];` (`:3799-3801`) — because it
//! is the one entry kind that contributes no message and still draws: `renderSessionItems` hands it
//! to `addCustomEntryToChat` (`:3717-3719`), which resolves
//! `extensionRunner.getEntryRenderer(entry.customType)` and builds a `CustomEntryComponent`
//! (`:3570-3590`) exactly as the live `entry_appended` arm does (`:3217-3218`).
//!
//! cyrup's [`crate::AgentSession::replay_items`] walked the projected MESSAGE list, and
//! `push_as_raw` has no arm for `KnownEntry::Custom` (`cyrup-session/src/context.rs`), so a custom
//! entry could not reach a front-end on `/resume` at all — `cyrup-intercom`'s inbound-message card
//! (`crates/cyrup-intercom/src/extension.rs`) is the shipped consumer that lost it.
//!
//! The second test is the half that a message-shaped fix would get wrong: pi replays custom entries
//! from `buildContextEntries()`, so a compaction's admission rule (`session-manager.ts:418-453`)
//! governs them exactly as it governs messages.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use cyrup_core::{Content, Message};
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use cyrup_session::manager::{NewSessionOpts, SessionManager};
use serde_json::{Value, json};
use tempfile::TempDir;

use crate::{ReplayItem, SessionBuilder, SessionConfig};

/// A session whose tree is exactly `entries`, built through the prebuilt-manager path so the test
/// owns the entry sequence (the runtime fork path, `SessionBuilder::with_manager`).
async fn session_over(
    tmp: &TempDir,
    build: impl FnOnce(&mut SessionManager),
) -> Arc<crate::AgentSession> {
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();

    let mut mgr = SessionManager::in_memory(&cwd, NewSessionOpts::default()).unwrap();
    build(&mut mgr);

    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true);
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    SessionBuilder::new(provider, cfg)
        .with_manager(mgr)
        .build()
        .await
        .unwrap()
        .into_shared()
}

fn user(text: &str) -> Message {
    Message::User {
        content: vec![Content::text(text)],
        timestamp: 0,
    }
}

/// The `customType`s of the `CustomEntry` items in a replay stream, in stream order.
fn custom_entry_types(items: &[ReplayItem]) -> Vec<String> {
    items
        .iter()
        .filter_map(|i| match i {
            ReplayItem::CustomEntry(v) => Some(
                v.get("customType")
                    .and_then(Value::as_str)
                    .unwrap_or("<no customType>")
                    .to_string(),
            ),
            _ => None,
        })
        .collect()
}

/// The user text of each `Message` item, so the interleaving is checked against something.
fn user_texts(items: &[ReplayItem]) -> Vec<String> {
    items
        .iter()
        .filter_map(|i| match i {
            ReplayItem::Message(m) => match m.as_ref() {
                cyrup_session::agent_message::AgentMessage::Core(Message::User {
                    content, ..
                }) => Some(
                    content
                        .iter()
                        .filter_map(|c| match c {
                            Content::Text { text, .. } => Some(text.to_string()),
                            _ => None,
                        })
                        .collect::<String>(),
                ),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// A `custom` entry replays, IN BRANCH ORDER, carrying the same JSON the live `entry_appended`
/// event carries — `type`, `customType` and `data` — which is what an entry renderer is handed
/// (`new CustomEntryComponent(entry, renderer)`, `interactive-mode.ts:3575`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_custom_entry_replays_between_the_messages_that_surround_it() {
    let tmp = TempDir::new().unwrap();
    let session = session_over(&tmp, |mgr| {
        mgr.append_message(user("before")).unwrap();
        mgr.append_custom_entry("intercom-inbound", Some(json!({ "from": "ada" })))
            .unwrap();
        mgr.append_message(user("after")).unwrap();
    })
    .await;

    let items = session.replay_items().await;

    assert_eq!(
        custom_entry_types(&items),
        vec!["intercom-inbound".to_string()],
        "the custom entry reached the replay stream: {items:#?}"
    );
    assert_eq!(
        user_texts(&items),
        vec!["before".to_string(), "after".to_string()],
        "and the messages around it are untouched: {items:#?}"
    );

    // Stream ORDER: pi's flat-map keeps the entry where the branch put it, so a front-end draws the
    // card between the two turns rather than at the end.
    let positions: Vec<&str> = items
        .iter()
        .map(|i| match i {
            ReplayItem::Message(_) => "message",
            ReplayItem::CustomEntry(_) => "custom-entry",
            ReplayItem::CacheMiss(_) => "cache-miss",
            ReplayItem::CompactionCost { .. } => "compaction-cost",
        })
        .collect();
    assert_eq!(
        positions,
        vec!["message", "custom-entry", "message"],
        "branch order preserved: {items:#?}"
    );

    let entry = items
        .iter()
        .find_map(|i| match i {
            ReplayItem::CustomEntry(v) => Some(v.clone()),
            _ => None,
        })
        .expect("the custom entry item");
    assert_eq!(
        entry.get("type").and_then(Value::as_str),
        Some("custom"),
        "the persisted serde tag, i.e. the shape `LiveHostServices::append_entry` puts on the live \
         wire: {entry}"
    );
    assert_eq!(
        entry.get("data"),
        Some(&json!({ "from": "ada" })),
        "the renderer's payload rides along whole: {entry}"
    );
    assert!(
        entry.get("id").and_then(Value::as_str).is_some(),
        "and the entry identity: {entry}"
    );
}

/// A compaction governs custom entries exactly as it governs messages — pi replays them out of
/// `buildContextEntries()` (`session-manager.ts:418-453` @v0.84.4), so the entry before
/// `firstKeptEntryId` is summarized away and the kept / post-compaction ones survive.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compaction_admission_applies_to_custom_entries_too() {
    let tmp = TempDir::new().unwrap();
    let session = session_over(&tmp, |mgr| {
        mgr.append_custom_entry("dropped", None).unwrap();
        mgr.append_message(user("summarized away")).unwrap();
        let kept = mgr.append_custom_entry("kept-before", None).unwrap();
        mgr.append_compaction("the summary".into(), kept, 1_234, None, None, false)
            .unwrap();
        mgr.append_custom_entry("after", None).unwrap();
    })
    .await;

    let items = session.replay_items().await;

    assert_eq!(
        custom_entry_types(&items),
        vec!["kept-before".to_string(), "after".to_string()],
        "the pre-`firstKeptEntryId` entry is admitted no more than the message beside it: \
         {items:#?}"
    );
    assert!(
        user_texts(&items).iter().all(|t| t != "summarized away"),
        "sanity: the message half of the admission rule is unchanged: {items:#?}"
    );
    // The compaction summary is the head item, and the kept custom entry follows it — pi's
    // `contextEntries = [compaction, ...kept, ...after]`.
    assert!(
        matches!(items.first(), Some(ReplayItem::Message(m))
            if matches!(m.as_ref(), cyrup_session::agent_message::AgentMessage::CompactionSummary(_))),
        "the governing compaction still heads the stream: {items:#?}"
    );
}
