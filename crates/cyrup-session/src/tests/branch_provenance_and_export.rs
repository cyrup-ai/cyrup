//! Branch-summary provenance and output cap, and the linearised branch export.
//!
//! * SESS-063 — `branch_with_summary` records the PRE-navigation leaf as `fromId`
//!   (`session-manager.ts:1603-1604` @v0.87.1, since v0.84.3).
//! * SESS-060 — the branch-summary request caps output at `min(4096, model.maxTokens)`, zero meaning
//!   unbounded (`branch-summarization.ts:345` @v0.87.1, since v0.85.0).
//! * SESS-058 — `export_branch_jsonl` is pi's `serializeSessionBranch` (`core/session-export.ts:9-29`
//!   @v0.87.1): a fresh header, then the current branch re-chained from `null`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cyrup_core::{CancelToken, Content, EntryId, Message, StopReason};
use cyrup_provider::faux::{FauxProvider, FauxResponseStep, faux_assistant_message, faux_text};
use serde_json::Value;

use crate::compaction::summarize::ProviderSummarizer;
use crate::{BranchSummarySettings, Compactor, NewSessionOpts, NoHooks, SessionManager};

fn user(s: &str) -> Message {
    Message::User {
        content: vec![Content::text(s)],
        timestamp: 0,
    }
}

fn assistant(s: &str) -> Message {
    Message::Assistant(faux_assistant_message(vec![faux_text(s)], StopReason::Stop))
}

fn session() -> SessionManager {
    SessionManager::in_memory(&PathBuf::from("/proj/lows"), NewSessionOpts::default()).unwrap()
}

// ------------------------------------------------------------------ SESS-063 -------------------

/// Leaf at `B`, navigate to `A` with a summary: the entry hangs under `A` and names `B` as `fromId`.
#[test]
fn a_branch_summary_names_the_leaf_it_left_as_from_id() {
    let mut m = session();
    let a = m.append_message(user("a")).unwrap();
    let b = m.append_message(assistant("b")).unwrap();
    let id = m
        .branch_with_summary(Some(&a), "left b".into(), None, None, false)
        .unwrap();
    let entry = serde_json::to_value(m.entry(&id).unwrap()).unwrap();
    assert_eq!(entry["fromId"], Value::String(b.to_string()));
    assert_eq!(entry["parentId"], Value::String(a.to_string()));
    assert_eq!(m.leaf_id(), Some(&id));
}

/// With no leaf before the navigation pi records `"root"` (`this.leafId ?? "root"`).
#[test]
fn a_branch_summary_from_an_empty_leaf_names_root() {
    let mut m = session();
    let a = m.append_message(user("a")).unwrap();
    m.reset_leaf();
    let id = m
        .branch_with_summary(Some(&a), "from nowhere".into(), None, None, false)
        .unwrap();
    let entry = serde_json::to_value(m.entry(&id).unwrap()).unwrap();
    assert_eq!(entry["fromId"], "root");
}

// ------------------------------------------------------------------ SESS-060 -------------------

/// Run a real branch summary against a faux model reporting `max_tokens`, returning the
/// `maxTokens` the provider request carried.
async fn branch_summary_max_tokens(max_tokens: u64) -> Option<u64> {
    let faux = Arc::new(FauxProvider::new());
    let seen: Arc<Mutex<Vec<Option<u64>>>> = Arc::default();
    let sink = seen.clone();
    faux.set_response_steps(vec![FauxResponseStep::factory(
        move |_ctx, opts, _state, _model| {
            sink.lock().unwrap().push(opts.max_tokens);
            faux_assistant_message(vec![faux_text("BRANCH SUMMARY")], StopReason::Stop)
        },
    )]);
    let mut model = faux.model().clone();
    model.max_tokens = max_tokens;
    let compactor = Compactor::new(
        ProviderSummarizer::new(faux.clone(), model.clone()),
        NoHooks,
    );

    let mut m = session();
    m.append_message(user("shared")).unwrap();
    let shared = m.append_message(assistant("shared answer")).unwrap();
    m.append_message(user("abandoned question")).unwrap();
    let abandoned = m.append_message(assistant("abandoned answer")).unwrap();
    compactor
        .run_branch_summary(
            &mut m,
            &model,
            shared,
            Some(abandoned),
            true,
            &BranchSummarySettings {
                reserve_tokens: 16384,
                skip_prompt: false,
            },
            CancelToken::new(),
        )
        .await
        .unwrap()
        .expect("a summary is appended");
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "one summarization call");
    seen[0]
}

#[tokio::test]
async fn the_branch_summary_cap_is_min_4096_and_the_models_max_tokens() {
    assert_eq!(branch_summary_max_tokens(8192).await, Some(4096));
    assert_eq!(branch_summary_max_tokens(1000).await, Some(1000));
    // `model.maxTokens > 0 ? … : Number.POSITIVE_INFINITY` — an unreported limit is uncapped.
    assert_eq!(branch_summary_max_tokens(0).await, Some(4096));
}

// ------------------------------------------------------------------ SESS-058 -------------------

/// Two branches, leaf on the second, plus an entry cyrup keeps verbatim: only the second branch is
/// written, re-chained from `null`, under a fresh header without `parentSession`.
#[test]
fn the_branch_export_is_the_current_branch_rechained_under_a_fresh_header() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("s.jsonl");
    // A stored header from long ago, with a parent session, and one extension entry of a type
    // cyrup does not model (kept as `Entry::Unknown`).
    let lines = [
        r#"{"type":"session","version":3,"id":"0197a0a0-0000-7000-8000-000000000001","timestamp":"2020-01-01T00:00:00.000Z","cwd":"/proj/lows","parentSession":"/old/parent.jsonl"}"#,
        r#"{"type":"message","id":"u1","parentId":null,"timestamp":"2020-01-01T00:00:01.000Z","message":{"role":"user","content":"first","timestamp":0}}"#,
        r#"{"type":"message","id":"x1","parentId":"u1","timestamp":"2020-01-01T00:00:02.000Z","message":{"role":"user","content":"abandoned","timestamp":0}}"#,
        r#"{"type":"future_kind","id":"f1","parentId":"u1","timestamp":"2020-01-01T00:00:03.000Z","payload":1}"#,
        r#"{"type":"message","id":"u2","parentId":"f1","timestamp":"2020-01-01T00:00:04.000Z","message":{"role":"user","content":"kept","timestamp":0}}"#,
    ];
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    let m = SessionManager::open(&path).unwrap();
    assert_eq!(m.leaf_id(), Some(&EntryId::from("u2")));

    let mut buf = Vec::new();
    m.export_branch_jsonl(&mut buf).unwrap();
    let out: Vec<Value> = String::from_utf8(buf)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();

    let header = &out[0];
    assert_eq!(header["type"], "session");
    assert_eq!(header["version"], 3);
    assert_eq!(header["id"], "0197a0a0-0000-7000-8000-000000000001");
    assert_eq!(header["cwd"], "/proj/lows");
    assert!(header.get("parentSession").is_none(), "{header}");
    assert_ne!(
        header["timestamp"], "2020-01-01T00:00:00.000Z",
        "a fresh export timestamp"
    );

    let ids: Vec<&str> = out[1..].iter().map(|e| e["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["u1", "f1", "u2"], "the abandoned `x1` is left out");
    let parents: Vec<&Value> = out[1..].iter().map(|e| &e["parentId"]).collect();
    assert_eq!(
        parents,
        [&Value::Null, &Value::from("u1"), &Value::from("f1")]
    );
    assert_eq!(
        out[2]["payload"], 1,
        "the verbatim entry keeps its own fields"
    );

    // The whole-tree dump is unchanged: every entry, the stored header.
    let mut whole = Vec::new();
    m.export_jsonl(&mut whole).unwrap();
    let whole = String::from_utf8(whole).unwrap();
    assert!(whole.contains("\"x1\"") && whole.contains("parentSession"));
}
