//! A **deferred** assistant entry stays a first-class entry in the tree but contributes NOTHING to
//! the LLM context.
//!
//! Pi, `v0.84.1 packages/agent/src/harness/session/context.ts:71-73`
//! (`sessionEntryToContextMessages`, 100-line file):
//!
//! ```text
//! if (entry.type === "message") {
//!     if (entry.message.role === "assistant" && entry.message.stopReason === "deferred") return [];
//!     return [entry.message];
//! }
//! ```
//!
//! Why it matters: a deferred turn's `content` is `[]` (`v0.84.1 ai/src/providers/faux.ts:293-296`) —
//! it is a receipt carrying a `DeferredHandle`, not a turn. Feeding an empty assistant message to
//! the model corrupts the user/assistant alternation, which is exactly why Pi drops it from context
//! while keeping it in the tree (its reducer hard-fails a session whose deferred entry has no
//! handle, `v0.84.1 agent/src/harness/reducer.ts:274-281`).
//!
//! Workspace clippy DENIES `unwrap_used`/`expect_used`/`panic`/`indexing_slicing`, hence the
//! file-level allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::io::Write;

use cyrup_core::Message;
use crate::agent_message::MessageRole;
use crate::context::context_message_role;
use crate::entry::Entry;
use crate::manager::SessionManager;

const HEADER: &str = r#"{"type":"session","version":3,"id":"01890000-0000-7000-8000-000000000002","timestamp":"2026-08-08T00:00:00.000Z","cwd":"/tmp/pi-session"}"#;

fn user(id: &str, parent: Option<&str>, text: &str) -> String {
    let parent = parent.map_or("null".to_string(), |p| format!("\"{p}\""));
    format!(
        r#"{{"type":"message","id":"{id}","parentId":{parent},"timestamp":"2026-08-08T00:00:00.000Z","message":{{"role":"user","content":[{{"type":"text","text":"{text}"}}],"timestamp":1754611200000}}}}"#
    )
}

/// An assistant entry with EMPTY content, parameterised only by the `stopReason` tail — so the
/// deferred case and the mirror case differ in exactly one token.
fn assistant(id: &str, parent: &str, tail: &str) -> String {
    format!(
        r#"{{"type":"message","id":"{id}","parentId":"{parent}","timestamp":"2026-08-08T00:00:00.000Z","message":{{"role":"assistant","content":[],"api":"openai-responses","provider":"openai","model":"gpt-5.5","usage":{{"input":10,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":10,"cost":{{"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0,"total":0.0}}}},{tail},"timestamp":1754611200000}}}}"#
    )
}

const DEFERRED_TAIL: &str = r#""stopReason":"deferred","deferred":{"provider":"openai","modelId":"gpt-5.5","api":"openai-responses","id":"resp_ctx"}"#;
const SETTLED_TAIL: &str = r#""stopReason":"stop""#;

/// Write a `[user, assistant(tail), user]` session and open it.
fn open_session(tail: &str) -> (tempfile::TempDir, SessionManager) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let mut f = std::fs::File::create(&path).expect("create");
    for line in [
        HEADER.to_string(),
        user("fffffff1", None, "start the job"),
        assistant("fffffff2", "fffffff1", tail),
        user("fffffff3", Some("fffffff2"), "any news?"),
    ] {
        writeln!(f, "{line}").expect("write");
    }
    f.flush().expect("flush");
    let mgr = SessionManager::import_jsonl(&path).expect("import");
    (dir, mgr)
}

fn roles(messages: &[Message]) -> Vec<&'static str> {
    messages
        .iter()
        .map(|m| match m {
            Message::User { .. } => "user",
            Message::Assistant(_) => "assistant",
            Message::ToolResult { .. } => "toolResult",
        })
        .collect()
}

/// A deferred assistant entry contributes NO context message (`context.ts:72`), while the entries
/// around it are untouched.
#[test]
fn deferred_assistant_is_excluded_from_llm_context() {
    let (_dir, mgr) = open_session(DEFERRED_TAIL);
    let ctx = mgr.build_context();
    assert_eq!(
        roles(&ctx.messages),
        vec!["user", "user"],
        "the deferred receipt must not become an empty assistant turn: {:?}",
        roles(&ctx.messages)
    );
}

/// MIRROR — the SAME entry, empty content and all, with `stopReason: "stop"` instead. It is a real
/// (if empty) turn and Pi keeps it: `context.ts:73` returns `[entry.message]`. This is what proves
/// the exclusion keys on the stop reason and not on "assistant" or on "empty content".
#[test]
fn mirror_settled_empty_assistant_stays_in_llm_context() {
    let (_dir, mgr) = open_session(SETTLED_TAIL);
    let ctx = mgr.build_context();
    assert_eq!(
        roles(&ctx.messages),
        vec!["user", "assistant", "user"],
        "a settled turn must stay in context even with empty content: {:?}",
        roles(&ctx.messages)
    );
}

/// The exclusion is one answer across every projection in `context.rs`: `context_message_role` —
/// the no-clone classifier the compaction cut-point scan and token estimate run on — must agree
/// with the message projection (Pi's single predicate is
/// `sessionEntryToContextMessages(entry).length === 0`).
#[test]
fn deferred_entry_is_context_invisible_to_the_cut_point_classifier() {
    let (_dir, mgr) = open_session(DEFERRED_TAIL);
    let deferred = mgr
        .entries()
        .iter()
        .find(|e| e.id().as_str() == "fffffff2")
        .expect("deferred entry present in the tree");

    // Still a first-class, INTERPRETED entry — not an opaque preserved blob.
    assert!(matches!(deferred, Entry::Known(_)), "deferred entry must parse as a known entry");
    assert_eq!(deferred.type_tag().as_deref(), Some("message"));

    assert_eq!(
        context_message_role(deferred),
        None,
        "deferred entry must be context-invisible to the cut-point classifier too"
    );
}

/// MIRROR for the classifier: the settled twin classifies as an assistant.
#[test]
fn mirror_settled_entry_classifies_as_assistant() {
    let (_dir, mgr) = open_session(SETTLED_TAIL);
    let settled = mgr
        .entries()
        .iter()
        .find(|e| e.id().as_str() == "fffffff2")
        .expect("settled entry present");
    assert_eq!(context_message_role(settled), Some(MessageRole::Assistant));
}
