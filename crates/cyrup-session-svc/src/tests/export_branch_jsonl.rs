//! SESS-058 / DRIFT-055 — `AgentSession::export_to_jsonl` (behind `/export <file>.jsonl`, `/share`
//! and RPC `export_jsonl`) writes the CURRENT BRANCH, linearised, under a fresh header.
//!
//! pi `exportToJsonl` → `exportSessionToJsonl` → `serializeSessionBranch`
//! (`agent-session.ts:3934-3935`, `core/session-export.ts:9-29` @v0.87.1): a new
//! `{type, version, id, timestamp: now, cwd}` header, then `getBranch()` with each `parentId`
//! rewritten to the previous entry's id. cyrup exported the whole tree, original parents and the
//! stored header, so a transcript (or a share upload) carried every abandoned branch.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use crate::{NavigateTreeOptions, SessionBuilder, SessionConfig};
use cyrup_core::StopReason;
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text};
use serde_json::Value;
use tempfile::TempDir;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

#[tokio::test]
async fn jsonl_export_is_the_current_branch_rechained_under_a_fresh_header() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let mut cfg = SessionConfig::new(cwd.clone(), agent_dir);
    cfg.trust_override = Some(true);
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("answer one")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("abandoned answer")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("answer three")], StopReason::Stop),
    ]);
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, cfg)
        .build()
        .await
        .expect("build");

    let _ = session.prompt("question one").await.unwrap();
    session.wait_for_idle().await;
    let _ = session.prompt("abandoned question").await.unwrap();
    session.wait_for_idle().await;
    // Back to before the second question (navigating to a user message leaves the leaf on its
    // parent), then continue on a new branch.
    let second_user = session.user_messages_for_forking().await[1]
        .entry_id
        .clone();
    session
        .navigate_tree(second_user, NavigateTreeOptions::default())
        .await
        .unwrap();
    let _ = session.prompt("question three").await.unwrap();
    session.wait_for_idle().await;

    // The whole tree still holds the abandoned branch — the export must not.
    let whole_tree = session.entries_json().await;
    assert!(
        whole_tree
            .iter()
            .any(|e| e.to_string().contains("abandoned question")),
        "precondition: the tree has two branches"
    );
    let last_entry_ts = whole_tree
        .iter()
        .filter_map(|e| e["timestamp"].as_str())
        .map(|t| OffsetDateTime::parse(t, &Rfc3339).unwrap())
        .max()
        .unwrap();

    let jsonl = session.export_to_jsonl(None).await.unwrap().expect("jsonl");
    let lines: Vec<Value> = jsonl
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();

    let header = &lines[0];
    assert_eq!(header["type"], "session");
    assert_eq!(header["version"], 3);
    // `{type, version, id, timestamp, cwd}` and nothing else — no `parentSession`.
    let mut keys: Vec<&str> = header
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["cwd", "id", "timestamp", "type", "version"],
        "{header}"
    );
    assert_eq!(header["id"], session.session_id().to_string());
    assert_eq!(header["cwd"], cwd.to_string_lossy().as_ref());
    let exported_at =
        OffsetDateTime::parse(header["timestamp"].as_str().unwrap(), &Rfc3339).unwrap();
    assert!(
        exported_at >= last_entry_ts,
        "the header is stamped at export time, not at session creation: {header}"
    );

    assert!(
        !jsonl.contains("abandoned question") && !jsonl.contains("abandoned answer"),
        "the abandoned branch must not be exported:\n{jsonl}"
    );
    for text in [
        "question one",
        "answer one",
        "question three",
        "answer three",
    ] {
        assert!(jsonl.contains(text), "{text} missing:\n{jsonl}");
    }
    // One chain from `null`: every entry's parent is the line before it.
    let mut previous = Value::Null;
    for entry in &lines[1..] {
        assert_eq!(entry["parentId"], previous, "{entry}");
        previous = entry["id"].clone();
    }
}
