//! TUI-120 — `/tree` over a pi v0.86+ session: `usage` and `context_edit` entries, and the
//! settings/bookkeeping class pi's default view hides.
//!
//! ```ts
//! // pi v0.87.1 coding-agent/src/modes/interactive/components/tree-selector.ts:339-386
//! this.filteredNodes = this.flatNodes.filter((flatNode) => {
//!     const entry = flatNode.node.entry;
//!     if (entry.type === "usage") return false;
//!     …
//!     const isSettingsEntry =
//!         entry.type === "label" || entry.type === "context_edit" || entry.type === "custom" ||
//!         entry.type === "model_change" || entry.type === "thinking_level_change" ||
//!         entry.type === "session_info";
//!     switch (this.filterMode) {
//!         case "user-only": passesFilter = entry.type === "message" && entry.message.role === "user"; break;
//!         case "no-tools": passesFilter = !isSettingsEntry && !(… role === "toolResult"); break;
//!         case "labeled-only": passesFilter = flatNode.node.label !== undefined; break;
//!         case "all": passesFilter = true; break;
//!         default: passesFilter = !isSettingsEntry; break;
//!     }
//! ```
//!
//! cyrup's `default` and `all` kept every row, so a pi session's cache-warm `usage` entries and its
//! `context_edit`s each showed up as a bare row in the default view (`usage` as `(entry)`), and so did
//! every label, custom entry and model/thinking change. The session-level case drives the real
//! `/tree` command against a resumed pi-written session file and reads the frame.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::Path;
use std::sync::Arc;

use super::harness::{buf_text, ctrl};
use crate::crossterm::event::KeyCode;
use crate::{
    App, AppCommand, FilterMode, SelectorKind, TreeEntryRole, TreeKind, TreeNode, TreeSelector,
    UiTheme,
};
use cyrup_core::{Message, StopReason};
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text};
use cyrup_session_svc::{AgentSession, SessionBuilder, SessionConfig, SessionTarget};
use ratatui::backend::TestBackend;

/// `u1 → a1 → usage → usage(malformed) → thinking_level_change → context_edit(omit a1) → u2`, as
/// pi writes it.
///
/// **The first `usage` row is COMPLETE** (`kind`/`provider`/`model`/`usage`, pi `UsageEntry`
/// @v1.0.4), so it promotes to `KnownEntry::Usage` and exercises `dag_display`'s KNOWN arm. SESS-051
/// promoted the variant; before it, every `usage` row — including this fixture's original one, which
/// carried no `kind` — demoted to `Entry::Unknown` and this test passed through the `Entry::Unknown`
/// guard instead. It would have kept passing after the promotion, for the wrong reason, so the
/// fixture had to be re-pointed.
///
/// **The second is deliberately malformed** (no `kind`), so it still demotes, pinning the degraded
/// path `dag_display` keeps beside the known arm. Both must be hidden in every filter mode.
///
/// The complete row needs its `usage.cost` block: `cyrup_core::Usage::cost` is REQUIRED (pi always
/// writes it), so a `usage` row without one fails `from_value::<KnownEntry>` and demotes. The
/// first attempt at this fixture omitted it and the re-proof below silently did not fire — which is
/// the same trap, one level down.
fn pi_session(dir: &Path, cwd: &Path) -> std::path::PathBuf {
    let ts = "2026-01-01T00:00:00.000Z";
    let assistant = serde_json::to_value(Message::Assistant(faux_assistant_message(
        vec![faux_text("first answer")],
        StopReason::Stop,
    )))
    .unwrap();
    let lines = [
        serde_json::json!({"type": "session", "version": 3, "id": "0199aaaa-bbbb-7ccc-8ddd-eeeeffff0003", "timestamp": ts, "cwd": cwd}),
        serde_json::json!({"type": "message", "id": "u1", "parentId": null, "timestamp": ts, "message": {"role": "user", "content": "first question", "timestamp": 1}}),
        serde_json::json!({"type": "message", "id": "a1", "parentId": "u1", "timestamp": ts, "message": assistant}),
        serde_json::json!({"type": "usage", "id": "w1", "parentId": "a1", "timestamp": ts, "kind": "cache_warm", "provider": "anthropic", "model": "claude-sonnet-5", "usage": {"input": 10, "output": 1, "cacheRead": 9000, "cacheWrite": 0, "totalTokens": 9011, "cost": {"input": 0, "output": 0.000015, "cacheRead": 0.0027, "cacheWrite": 0, "total": 0.002715}}}),
        serde_json::json!({"type": "usage", "id": "w2", "parentId": "w1", "timestamp": ts, "usage": {"input": 10, "output": 0, "cacheRead": 9000, "cacheWrite": 0, "totalTokens": 9010}}),
        serde_json::json!({"type": "thinking_level_change", "id": "t1", "parentId": "w2", "timestamp": ts, "thinkingLevel": "high"}),
        serde_json::json!({"type": "context_edit", "id": "e1", "parentId": "t1", "timestamp": ts, "targetId": "a1", "replacement": null}),
        serde_json::json!({"type": "message", "id": "u2", "parentId": "e1", "timestamp": ts, "message": {"role": "user", "content": "second question", "timestamp": 2}}),
    ];
    let path = dir.join("pi-v087.jsonl");
    let text: Vec<String> = lines.iter().map(|v| v.to_string()).collect();
    std::fs::write(&path, text.join("\n") + "\n").unwrap();
    path
}

async fn resumed(dir: &Path) -> Arc<AgentSession> {
    let cwd = dir.join("project");
    let agent_dir = dir.join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let path = pi_session(dir, &cwd);
    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true);
    cfg.target = SessionTarget::Resume(path);
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    Arc::new(SessionBuilder::new(provider, cfg).build().await.unwrap())
}

/// The row's Verify, through the production `/tree` command: in `default` neither the `usage` nor
/// the `context_edit` (nor the thinking change) is a row; `all` brings the `context_edit` back with
/// pi's `[context omit: <targetId>]` text; NEITHER `usage` entry — the promoted one or the
/// malformed one — is in any view at all.
#[tokio::test]
async fn tree_hides_usage_always_and_context_edit_outside_the_all_filter() {
    let dir = tempfile::tempdir().unwrap();
    let session = resumed(dir.path()).await;
    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    app.execute_command(AppCommand::OpenSelector(SelectorKind::Tree), &session, None)
        .await;
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Tree));
    app.draw().unwrap();

    let default_view = buf_text(&app);
    assert!(default_view.contains("Filter: default"), "{default_view}");
    // Presence first, so the absences below are not a blank frame passing vacuously.
    assert!(
        default_view.contains("user: first question")
            && default_view.contains("assistant: first answer")
            && default_view.contains("user: second question"),
        "the conversation rows are drawn:\n{default_view}"
    );
    for hidden in ["(entry)", "[context omit", "thinking → high"] {
        assert!(
            !default_view.contains(hidden),
            "`{hidden}` is a row in the default view:\n{default_view}"
        );
    }

    // `app.tree.filter.all` — `ctrl+a` (`core/keybindings.ts`).
    app.handle_input(&ctrl(KeyCode::Char('a')));
    app.draw().unwrap();
    let all_view = buf_text(&app);
    assert!(all_view.contains("Filter: all"), "{all_view}");
    assert!(
        all_view.contains("[context omit: a1]") && all_view.contains("thinking → high"),
        "`all` shows the bookkeeping rows, labelled as pi labels them:\n{all_view}"
    );
    assert!(
        !all_view.contains("(entry)"),
        "a `usage` entry is dropped before any filter mode is consulted (`:341`), whether it \
         promoted to `KnownEntry::Usage` or demoted to `Entry::Unknown`:\n{all_view}"
    );
}

fn kinded(id: &str, label: &str, kind: TreeKind) -> TreeNode {
    let mut n = TreeNode::message(id, 0, label);
    n.kind = kind;
    n
}

/// Every filter mode against every class, on the selector itself.
#[test]
fn each_filter_mode_keeps_what_pis_apply_filter_keeps() {
    let mut labeled_setting = kinded("l", "[context replace: a]", TreeKind::Settings);
    labeled_setting.user_label = Some("kept".to_string());
    let nodes = vec![
        kinded("u", "user: hi", TreeKind::Message),
        kinded("a", "assistant: hello", TreeKind::Message),
        kinded("t", "[read]", TreeKind::ToolGroup),
        kinded("m", "model → opus", TreeKind::ModelChange),
        kinded("k", "thinking → high", TreeKind::ThinkingChange),
        kinded("c", "compaction", TreeKind::Compaction),
        kinded("x", "custom note", TreeKind::Settings),
        labeled_setting,
        kinded("w", "(entry)", TreeKind::Usage),
    ];
    let ids = |mode: FilterMode| {
        let mut sel = TreeSelector::new(nodes.clone());
        sel.set_filter(mode);
        sel.visible_ids()
    };
    let v = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(ids(FilterMode::Default), v(&["u", "a", "t", "c"]));
    assert_eq!(ids(FilterMode::NoTools), v(&["u", "a", "c"]));
    assert_eq!(ids(FilterMode::UserOnly), v(&["u"]));
    assert_eq!(ids(FilterMode::LabeledOnly), v(&["l"]));
    assert_eq!(
        ids(FilterMode::All),
        v(&["u", "a", "t", "m", "k", "c", "x", "l"])
    );
}

/// `theme.fg("dim", `[context ${…}: ${entry.targetId}]`)` (`tree-selector.ts:844-846`): a settings
/// row is dim end to end — the `[…]` shape must not be read as a custom message's coloured label.
#[test]
fn a_context_edit_row_is_dim() {
    assert_eq!(
        TreeEntryRole::classify(TreeKind::Settings, "[context omit: a1]"),
        TreeEntryRole::Dim
    );
}
