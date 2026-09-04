//! Conformance tests for arch-04 / A-04-1..10 (sessions & branching).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::{Path, PathBuf};

use crate::agent_message::AgentMessage;
use crate::{Entry, KnownEntry, NewSessionOpts, SessionLayout, SessionManager, SessionSelector};
use cyrup_core::{AssistantMessage, Content, Message, StopReason, Usage};
use serde_json::Value;

// ----------------------------------------------------------------- helpers --------------------

fn layout(root: &Path, cwd: &Path) -> SessionLayout {
    SessionLayout::new(root.to_path_buf(), cwd.to_path_buf())
}

fn user(s: &str) -> Message {
    Message::User {
        content: vec![Content::text(s)],
        timestamp: 0,
    }
}

fn assistant(s: &str) -> Message {
    Message::Assistant(AssistantMessage {
        content: vec![Content::text(s)],
        provider: "faux".into(),
        model: "faux-1".into(),
        api: "faux".into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        timestamp: 0,
    })
}

fn first_text(m: &Message) -> String {
    let blocks = match m {
        Message::User { content, .. } | Message::ToolResult { content, .. } => content,
        Message::Assistant(a) => &a.content,
    };
    blocks
        .iter()
        .find_map(|b| match b {
            Content::Text { text, .. } => Some(text.to_string()),
            _ => None,
        })
        .unwrap_or_default()
}

// ----------------------------------------------------------------- A-04-1 ---------------------

#[test]
fn a04_1_linear_tree_valid_jsonl() {
    let root = tempfile::tempdir().unwrap();
    let cwd = PathBuf::from("/proj/alpha");
    let lay = layout(root.path(), &cwd);

    let mut m = SessionManager::create(&cwd, &lay, NewSessionOpts::default()).unwrap();
    let u = m.append_message(user("hello")).unwrap();
    let a = m.append_message(assistant("hi there")).unwrap();

    // File exists after the first assistant message (deferred flush).
    let file = m.session_file().unwrap().to_path_buf();
    assert!(
        file.exists(),
        "session file should exist after assistant message"
    );

    let text = std::fs::read_to_string(&file).unwrap();
    let mut lines = text.lines();

    // Line 1 = header.
    let header: Value = serde_json::from_str(lines.next().unwrap()).unwrap();
    assert_eq!(header["type"], "session");
    assert_eq!(header["version"], 3);

    // Lines 2+ = entries forming a linear parentId chain; valid JSONL throughout.
    let e1: Value = serde_json::from_str(lines.next().unwrap()).unwrap();
    let e2: Value = serde_json::from_str(lines.next().unwrap()).unwrap();
    assert_eq!(e1["type"], "message");
    assert!(
        e1["parentId"].is_null(),
        "first entry parentId must be null"
    );
    assert_eq!(e2["parentId"], serde_json::json!(u.as_str()));
    assert_eq!(m.leaf_id().unwrap(), &a);
}

// ----------------------------------------------------------------- A-04-2 ---------------------

#[test]
fn a04_2_branching_keeps_both_branches() {
    let root = tempfile::tempdir().unwrap();
    let cwd = PathBuf::from("/proj/branch");
    let lay = layout(root.path(), &cwd);

    let mut m = SessionManager::create(&cwd, &lay, NewSessionOpts::default()).unwrap();
    let u1 = m.append_message(user("q1")).unwrap();
    let _a1 = m.append_message(assistant("a1")).unwrap();
    let _u2 = m.append_message(user("q2")).unwrap();
    let a2 = m.append_message(assistant("a2")).unwrap();

    // Branch back to a1, start a different continuation.
    m.branch(&u1).unwrap();
    let u2b = m.append_message(user("q2-alt")).unwrap();
    let a2b = m.append_message(assistant("a2-alt")).unwrap();

    // Both branches live in one file; abandoned entries are intact.
    assert!(m.entry(&a2).is_some(), "abandoned branch entry intact");
    assert_eq!(m.leaf_id().unwrap(), &a2b, "leaf points to the new branch");

    // u1 now has two children (the original q2 and the alt q2).
    let kids = m.children(&u1);
    assert_eq!(kids.len(), 2);

    // On disk: all entries present (header + 6 entries = 7 lines).
    let text = std::fs::read_to_string(m.session_file().unwrap()).unwrap();
    assert_eq!(text.lines().count(), 7);

    // Active path is the new branch only.
    let path_ids: Vec<_> = m.branch_path(None).iter().map(|e| e.id()).collect();
    assert!(path_ids.contains(&u2b));
    assert!(!path_ids.contains(&a2));
}

// ----------------------------------------------------------------- A-04-3 ---------------------

#[test]
fn a04_3_build_context_with_compaction() {
    let root = tempfile::tempdir().unwrap();
    let cwd = PathBuf::from("/proj/compact");
    let lay = layout(root.path(), &cwd);

    let mut m = SessionManager::create(&cwd, &lay, NewSessionOpts::default()).unwrap();
    let _u1 = m.append_message(user("old-q")).unwrap();
    let _a1 = m.append_message(assistant("old-a")).unwrap();
    let u2 = m.append_message(user("keep-q")).unwrap();
    let _a2 = m.append_message(assistant("keep-a")).unwrap();
    let _comp = m
        .append_compaction(
            "SUMMARY-OF-OLD".to_string(),
            u2.clone(),
            1234,
            None,
            None,
            false,
        )
        .unwrap();
    let _u3 = m.append_message(user("new-q")).unwrap();
    let _a3 = m.append_message(assistant("new-a")).unwrap();

    let ctx = m.build_context();
    // summary first, then kept (keep-q, keep-a), then post-compaction (new-q, new-a).
    assert_eq!(ctx.messages.len(), 5);
    assert!(first_text(&ctx.messages[0]).contains("SUMMARY-OF-OLD"));
    assert_eq!(first_text(&ctx.messages[1]), "keep-q");
    assert_eq!(first_text(&ctx.messages[2]), "keep-a");
    assert_eq!(first_text(&ctx.messages[3]), "new-q");
    assert_eq!(first_text(&ctx.messages[4]), "new-a");

    // Full history intact on disk: 6 messages + 1 compaction = 7 entries.
    assert_eq!(m.entries().len(), 7);
    let reopened = SessionManager::open(m.session_file().unwrap()).unwrap();
    assert_eq!(reopened.entries().len(), 7);
}

// ----------------------------------------------------------------- A-04-4 ---------------------

#[test]
fn a04_4_fork_and_clone() {
    let root = tempfile::tempdir().unwrap();
    let cwd = PathBuf::from("/proj/src");
    let lay = layout(root.path(), &cwd);

    let mut src = SessionManager::create(&cwd, &lay, NewSessionOpts::default()).unwrap();
    src.append_message(user("q")).unwrap();
    src.append_message(assistant("a")).unwrap();
    let src_path = src.session_file().unwrap().to_path_buf();
    let src_entry_count = src.entries().len();

    // Fork into another project.
    let cwd2 = PathBuf::from("/proj/dst");
    let lay2 = layout(root.path(), &cwd2);
    let forked =
        SessionManager::fork_from(&src_path, &cwd2, &lay2, NewSessionOpts::default()).unwrap();
    let forked_path = forked.session_file().unwrap().to_path_buf();
    assert!(forked_path.exists());
    assert_ne!(forked_path, src_path);
    assert_eq!(
        forked.header().parent_session.as_deref(),
        Some(src_path.to_string_lossy().as_ref())
    );
    assert_eq!(forked.entries().len(), src_entry_count);

    // Source unchanged.
    let src_reopened = SessionManager::open(&src_path).unwrap();
    assert_eq!(src_reopened.entries().len(), src_entry_count);

    // Clone (createBranchedSession) re-roots the active path through the current leaf into a new
    // file IN PLACE; the previous file (src_path) is untouched on disk.
    let leaf = src.leaf_id().cloned().unwrap();
    let cloned_path = src
        .create_branched_session(&leaf, &lay)
        .unwrap()
        .expect("persisted branch returns a path");
    assert!(cloned_path.exists());
    assert_ne!(cloned_path, src_path);
    assert_eq!(
        src.header().parent_session.as_deref(),
        Some(src_path.to_string_lossy().as_ref())
    );
    assert_eq!(src.entries().len(), src_entry_count);
    assert_eq!(first_text_of_leaf(&src), "a");
}

fn first_text_of_leaf(m: &SessionManager) -> String {
    match m.leaf_entry() {
        Some(Entry::Known(KnownEntry::Message {
            message: AgentMessage::Core(m),
            ..
        })) => first_text(m),
        _ => String::new(),
    }
}

// ----------------------------------------------------------------- A-04-5 ---------------------

#[test]
fn a04_5_continue_most_recent() {
    let root = tempfile::tempdir().unwrap();
    let cwd = PathBuf::from("/proj/cont");
    let lay = layout(root.path(), &cwd);

    // None present → creates a fresh (empty) session.
    let fresh = SessionManager::continue_recent(&cwd, &lay).unwrap();
    assert!(fresh.entries().is_empty());
    assert!(fresh.leaf_id().is_none());

    // Create + populate a session.
    let mut m = SessionManager::create(&cwd, &lay, NewSessionOpts::default()).unwrap();
    m.append_message(user("q")).unwrap();
    let leaf = m.append_message(assistant("a")).unwrap();

    // Continue resumes the latest session at its leaf.
    let resumed = SessionManager::continue_recent(&cwd, &lay).unwrap();
    assert_eq!(resumed.leaf_id(), Some(&leaf));
}

// ----------------------------------------------------------------- A-04-6 ---------------------

#[test]
fn a04_6_ephemeral_writes_no_file() {
    let root = tempfile::tempdir().unwrap();
    let cwd = PathBuf::from("/proj/ephemeral");

    let mut m = SessionManager::in_memory(&cwd, NewSessionOpts::default()).unwrap();
    m.append_message(user("q")).unwrap();
    m.append_message(assistant("a")).unwrap();

    assert!(!m.is_persisted());
    assert!(m.session_file().is_none());
    assert_eq!(m.build_context().messages.len(), 2);

    // No files anywhere under the (untouched) root.
    let count = std::fs::read_dir(root.path())
        .map(|rd| rd.count())
        .unwrap_or(0);
    assert_eq!(count, 0, "ephemeral mode must not write any files");
}

// ----------------------------------------------------------------- A-04-7 ---------------------

#[test]
fn a04_7_export_import_roundtrip() {
    let root = tempfile::tempdir().unwrap();
    let cwd = PathBuf::from("/proj/export");
    let lay = layout(root.path(), &cwd);

    let mut m = SessionManager::create(&cwd, &lay, NewSessionOpts::default()).unwrap();
    m.append_message(user("q1")).unwrap();
    m.append_message(assistant("a1")).unwrap();
    let leaf = m.append_message(user("q2")).unwrap();

    let export_path = root.path().join("exported.jsonl");
    {
        let mut f = std::fs::File::create(&export_path).unwrap();
        m.export_jsonl(&mut f).unwrap();
    }

    let imported = SessionManager::import_jsonl(&export_path).unwrap();
    assert_eq!(imported.entries().len(), m.entries().len());
    assert_eq!(
        imported.leaf_id(),
        Some(&leaf),
        "import resumes at the same leaf"
    );
}

// ----------------------------------------------------------------- A-04-8 ---------------------

#[test]
fn a04_8_interop_shape_and_unknown_roundtrip() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("pi-like.jsonl");

    // A Pi-shaped session: documented header + a message entry + an unknown extension entry.
    let contents = concat!(
        r#"{"type":"session","version":3,"id":"11111111-1111-7111-8111-111111111111","timestamp":"2026-01-01T00:00:00Z","cwd":"/proj/x"}"#,
        "\n",
        r#"{"type":"message","id":"aaaa1111","parentId":null,"timestamp":"2026-01-01T00:00:01Z","message":{"role":"user","content":[{"type":"text","text":"hi"}],"timestamp":0}}"#,
        "\n",
        r#"{"type":"weird_ext","id":"bbbb2222","parentId":"aaaa1111","timestamp":"2026-01-01T00:00:02Z","payload":{"x":1}}"#,
        "\n",
    );
    std::fs::write(&file, contents).unwrap();

    let m = SessionManager::open(&file).unwrap();
    assert_eq!(m.entries().len(), 2);
    // Documented camelCase field shape survives load + index.
    assert!(matches!(
        m.entries()[0],
        Entry::Known(KnownEntry::Message { .. })
    ));
    assert!(matches!(m.entries()[1], Entry::Unknown(_)));

    // Unknown extension entry is skipped from context but preserved on export.
    assert_eq!(m.build_context().messages.len(), 1);

    let mut buf = Vec::new();
    m.export_jsonl(&mut buf).unwrap();
    let out = String::from_utf8(buf).unwrap();
    assert!(
        out.contains("\"weird_ext\""),
        "unknown type preserved verbatim"
    );
    assert!(
        out.contains("\"payload\""),
        "unknown payload preserved verbatim"
    );
    assert!(out.contains("\"parentId\":\"aaaa1111\""));

    // model_change entries serialize with Pi camelCase field names.
    let root2 = tempfile::tempdir().unwrap();
    let cwd = PathBuf::from("/proj/mc");
    let lay = layout(root2.path(), &cwd);
    let mut mc = SessionManager::create(&cwd, &lay, NewSessionOpts::default()).unwrap();
    mc.append_model_change("anthropic".into(), "claude".into())
        .unwrap();
    let line = mc.entries()[0].to_line().unwrap();
    let v: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["type"], "model_change");
    assert_eq!(v["modelId"], "claude");
    assert_eq!(v["provider"], "anthropic");
}

// ----------------------------------------------------------------- A-04-9 ---------------------

#[test]
fn a04_9_custom_excluded_custom_message_included() {
    let root = tempfile::tempdir().unwrap();
    let cwd = PathBuf::from("/proj/custom");
    let lay = layout(root.path(), &cwd);

    let mut m = SessionManager::create(&cwd, &lay, NewSessionOpts::default()).unwrap();
    m.append_message(user("q")).unwrap();
    m.append_message(assistant("a")).unwrap();
    m.append_custom_entry("ext_state", Some(serde_json::json!({"k": "v"})))
        .unwrap();
    m.append_custom_message("ext_msg", Value::String("injected".to_string()), true, None)
        .unwrap();

    let ctx = m.build_context();
    let texts: Vec<String> = ctx.messages.iter().map(first_text).collect();
    assert!(
        texts.contains(&"injected".to_string()),
        "CustomMessage must be in context"
    );
    assert!(
        !texts.iter().any(|t| t.contains("ext_state")),
        "CustomEntry must NOT be in context"
    );
    // 2 messages + 1 custom_message = 3 in context; the custom entry is excluded.
    assert_eq!(ctx.messages.len(), 3);
}

// ----------------------------------------------------------------- A-04-10 --------------------

#[test]
fn a04_10_corrupt_trailing_line_loads_valid_prefix() {
    use std::io::Write as _;

    let root = tempfile::tempdir().unwrap();
    let cwd = PathBuf::from("/proj/corrupt");
    let lay = layout(root.path(), &cwd);

    let mut m = SessionManager::create(&cwd, &lay, NewSessionOpts::default()).unwrap();
    m.append_message(user("q")).unwrap();
    m.append_message(assistant("a")).unwrap();
    let path = m.session_file().unwrap().to_path_buf();
    let good_count = m.entries().len();
    drop(m);

    // Append a truncated, unparseable final line (simulating a crash mid-append).
    {
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        f.write_all(b"{\"type\":\"message\",\"id\":\"zz").unwrap();
    }

    // Loads the valid prefix without panicking.
    let recovered = SessionManager::open(&path).unwrap();
    assert_eq!(
        recovered.entries().len(),
        good_count,
        "valid prefix recovered"
    );
}

// -------------------------------------------------- StopReason wire compatibility -------------

/// ADVERSARIAL guard for adding `StopReason::Pending` (PROV-010 / AGENT-014 / DRIFT-012): the
/// variant changes a SERIALIZED shape, so an existing on-disk session must keep loading unchanged.
///
/// The five pre-existing spellings are byte-identical before and after, so this asserts against
/// literal JSON rather than a round-trip — a round-trip would pass even if both directions had
/// shifted together.
#[test]
fn an_existing_session_jsonl_still_loads_after_the_pending_variant_was_added() {
    use std::io::Write as _;

    let root = tempfile::tempdir().unwrap();
    let cwd = PathBuf::from("/proj/wirecompat");
    let lay = layout(root.path(), &cwd);

    // The session file is deferred-flushed until an assistant message lands (see a04_10), so seed
    // one turn before hand-writing raw lines onto the end.
    let mut m = SessionManager::create(&cwd, &lay, NewSessionOpts::default()).unwrap();
    m.append_message(user("seed")).unwrap();
    m.append_message(assistant("seed")).unwrap();
    let path = m.session_file().unwrap().to_path_buf();
    let base = m.entries().len();
    drop(m);

    // Hand-written entries in the pre-change on-disk shape, one per settled stop reason.
    let wire = ["stop", "length", "toolUse", "error", "aborted"];
    {
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        for (i, w) in wire.iter().enumerate() {
            let line = serde_json::json!({
                "type": "message",
                "id": format!("m{i}"),
                "parentId": null,
                "timestamp": "2026-01-01T00:00:00.000Z",
                "message": {
                    "role": "assistant",
                    "stopReason": w,
                    "content": [{"type": "text", "text": format!("turn {i}")}],
                    "api": "faux",
                    "provider": "faux",
                    "model": "faux-1",
                    "usage": {
                        "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 0,
                        "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0,
                                 "cacheWrite": 0.0, "total": 0.0}
                    },
                    "timestamp": 0
                }
            });
            writeln!(f, "{line}").unwrap();
        }
    }

    let reopened = SessionManager::open(&path).unwrap();
    assert_eq!(
        reopened.entries().len(),
        base + wire.len(),
        "an old-shape session lost entries — the new variant broke the read path"
    );

    let got: Vec<StopReason> = reopened
        .entries()
        .iter()
        .filter_map(|e| match e {
            Entry::Known(KnownEntry::Message {
                message: AgentMessage::Core(m @ Message::Assistant(_)),
                ..
            }) if first_text(m).starts_with("turn ") => match m {
                Message::Assistant(a) => Some(a.stop_reason),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(
        got,
        vec![
            StopReason::Stop,
            StopReason::Length,
            StopReason::ToolUse,
            StopReason::Error,
            StopReason::Aborted,
        ],
        "every pre-existing wire spelling must still decode to the same variant"
    );

    // Re-export must reproduce the same bytes — no variant renamed itself on the way out.
    for (i, w) in wire.iter().enumerate() {
        let a = AssistantMessage {
            stop_reason: got[i],
            ..match assistant("x") {
                Message::Assistant(a) => a,
                _ => unreachable!(),
            }
        };
        assert_eq!(serde_json::to_value(a).unwrap()["stopReason"], *w);
    }
}

/// The interop gap the variant closes. A Pi-produced payload can carry `"stopReason":"pending"` —
/// `agent-loop.ts:314-318` emits `message_start` as `{...partialMessage}`, whose `stopReason` is
/// the `"pending"` seed every Pi stream function sets.
///
/// The pre-fix failure mode was neither "the load errors out" nor "the line is dropped": `Entry`
/// has an `Unknown(Value)` fallback for a known tag with an unparseable body, so the entry survived
/// in the FILE verbatim and round-tripped on export — but `build_context` only walks
/// `KnownEntry::Message` (context.rs:60,175), so the message was invisible to the model and to
/// every consumer that reads the transcript rather than the raw entries. Silent, and worse than a
/// drop, because the file kept looking intact. This test pins both halves: it decodes as a real
/// message AND reaches the built context.
#[test]
fn a_pi_pending_entry_is_no_longer_silently_dropped_on_load() {
    use std::io::Write as _;

    let root = tempfile::tempdir().unwrap();
    let cwd = PathBuf::from("/proj/pending-import");
    let lay = layout(root.path(), &cwd);

    let mut m = SessionManager::create(&cwd, &lay, NewSessionOpts::default()).unwrap();
    m.append_message(user("seed")).unwrap();
    m.append_message(assistant("seed")).unwrap();
    let path = m.session_file().unwrap().to_path_buf();
    let base = m.entries().len();
    drop(m);

    {
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        let line = serde_json::json!({
            "type": "message",
            "id": "p0",
            "parentId": null,
            "timestamp": "2026-01-01T00:00:00.000Z",
            "message": {
                "role": "assistant",
                "stopReason": "pending",
                "content": [{"type": "text", "text": "half a thou"}],
                "api": "anthropic-messages",
                "provider": "anthropic",
                "model": "claude-x",
                "usage": {
                    "input": 10, "output": 3, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 13,
                    "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0,
                             "cacheWrite": 0.0, "total": 0.0}
                },
                "timestamp": 0
            }
        });
        writeln!(f, "{line}").unwrap();
    }

    let reopened = SessionManager::open(&path).unwrap();
    assert_eq!(
        reopened.entries().len(),
        base + 1,
        "the pending entry was dropped instead of imported"
    );
    let last = reopened.entries().last().unwrap();
    match last {
        Entry::Known(KnownEntry::Message {
            message: AgentMessage::Core(Message::Assistant(a)),
            ..
        }) => {
            assert_eq!(a.stop_reason, StopReason::Pending);
            assert_eq!(serde_json::to_value(a).unwrap()["stopReason"], "pending");
        }
        other => panic!("expected an assistant message entry, got {other:?}"),
    }

    // The half that actually mattered: it reaches the built context. An `Entry::Unknown` would
    // have been skipped here while still occupying a line in the file.
    let ctx = reopened.build_context();
    assert!(
        ctx.messages.iter().any(|m| first_text(m) == "half a thou"),
        "the imported message never reached the context: {:?}",
        ctx.messages.iter().map(first_text).collect::<Vec<_>>()
    );
}

/// The strictness half of the same decision: there is deliberately NO `#[serde(other)]` fallback,
/// so a value outside Pi's closed union (`types.ts:391`) is still rejected rather than absorbed
/// into a catch-all that a `_ =>` success arm would then mistake for a completed turn. Behaviour is
/// unchanged from before the variant — asserted so a future "just make it tolerant" patch has to
/// argue with a test.
#[test]
fn an_unknown_stop_reason_is_still_rejected_not_absorbed() {
    assert!(serde_json::from_value::<StopReason>(serde_json::json!("someNewReason")).is_err());
    assert!(serde_json::from_value::<StopReason>(serde_json::json!("Pending")).is_err());
}

// ----------------------------------------------------------------- extras ---------------------

#[test]
fn reset_leaf_yields_empty_context_then_new_root() {
    let root = tempfile::tempdir().unwrap();
    let cwd = PathBuf::from("/proj/reset");
    let lay = layout(root.path(), &cwd);

    let mut m = SessionManager::create(&cwd, &lay, NewSessionOpts::default()).unwrap();
    m.append_message(user("q")).unwrap();
    m.append_message(assistant("a")).unwrap();

    m.reset_leaf();
    assert!(m.build_context().messages.is_empty());

    let new_root = m.append_message(user("fresh")).unwrap();
    assert!(m.entry(&new_root).unwrap().parent_id().is_none());
}

#[test]
fn selector_resolves_path_and_uuid_prefix() {
    let root = tempfile::tempdir().unwrap();
    let cwd = PathBuf::from("/proj/select");
    let lay = layout(root.path(), &cwd);

    let mut m = SessionManager::create(&cwd, &lay, NewSessionOpts::default()).unwrap();
    m.append_message(user("q")).unwrap();
    m.append_message(assistant("a")).unwrap();
    let path = m.session_file().unwrap().to_path_buf();
    let id = m.session_id().to_string();

    let by_path = crate::resolve(&SessionSelector::Path(path.clone()), &lay).unwrap();
    assert_eq!(by_path, path);

    let prefix = id.get(..8).unwrap().to_string();
    let by_uuid = crate::resolve(&SessionSelector::Uuid(prefix), &lay).unwrap();
    assert_eq!(by_uuid, path);

    let missing = crate::resolve(&SessionSelector::Uuid("ffffffff".into()), &lay);
    assert!(missing.is_err());
}

#[test]
fn v1_legacy_file_migrates_to_v3_on_load() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("legacy.jsonl");

    // A v1 file: header with no `version`, entries with no `id`/`parentId` (linear).
    let contents = concat!(
        r#"{"type":"session","id":"22222222-2222-7222-8222-222222222222","timestamp":"2025-01-01T00:00:00Z","cwd":"/proj/legacy"}"#,
        "\n",
        r#"{"type":"message","timestamp":"2025-01-01T00:00:01Z","message":{"role":"user","content":[{"type":"text","text":"old"}],"timestamp":0}}"#,
        "\n",
        r#"{"type":"message","timestamp":"2025-01-01T00:00:02Z","message":{"role":"assistant","content":[{"type":"text","text":"reply"}],"provider":"faux","model":"f","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}},"stopReason":"stop","timestamp":0}}"#,
        "\n",
    );
    std::fs::write(&file, contents).unwrap();

    let m = SessionManager::open(&file).unwrap();
    assert_eq!(
        m.header().version,
        Some(3),
        "header migrated to current version"
    );
    assert_eq!(m.entries().len(), 2);
    // Ids minted + linear parent chain established.
    assert!(m.entries()[0].parent_id().is_none());
    assert_eq!(m.entries()[1].parent_id(), Some(m.entries()[0].id()));
    // Migrated entries are interpretable → context has both messages.
    assert_eq!(m.build_context().messages.len(), 2);

    // The file was rewritten at v3 on load.
    let reopened = SessionManager::open(&file).unwrap();
    assert_eq!(reopened.header().version, Some(3));
}

#[test]
fn labels_and_session_name() {
    let root = tempfile::tempdir().unwrap();
    let cwd = PathBuf::from("/proj/labels");
    let lay = layout(root.path(), &cwd);

    let mut m = SessionManager::create(&cwd, &lay, NewSessionOpts::default()).unwrap();
    let u = m.append_message(user("q")).unwrap();
    m.append_message(assistant("a")).unwrap();
    m.append_label(&u, Some("important")).unwrap();
    m.append_session_info("My Session").unwrap();

    assert_eq!(m.label(&u), Some("important"));
    assert_eq!(m.session_name().as_deref(), Some("My Session"));
}

// --------------------------------------------- raw context projection (Pi buildContextEntries) --

/// [`SessionManager::build_context_raw`] is Pi's
/// `buildContextEntries().flatMap(sessionEntryToContextMessages)` (`session-manager.ts:441-453` +
/// `:383-408`) — the projection a UI replays a resumed session from. It must keep the
/// `compactionSummary` / `branchSummary` / `custom` / `bashExecution` roles that
/// [`SessionManager::build_context`] flattens to `user` at the LLM boundary (`convertToLlm`,
/// `messages.ts:148-195`); a front-end fed the flattened form draws the wrapper prose as something
/// the user typed.
#[test]
fn build_context_raw_keeps_the_roles_build_context_flattens() {
    let root = tempfile::tempdir().unwrap();
    let cwd = PathBuf::from("/proj/raw-context");
    let lay = layout(root.path(), &cwd);

    let mut m = SessionManager::create(&cwd, &lay, NewSessionOpts::default()).unwrap();
    let u = m.append_message(user("start")).unwrap();
    m.append_message(assistant("ok")).unwrap();
    m.append_compaction(
        "we did a refactor".into(),
        u.clone(),
        42_000,
        None,
        None,
        false,
    )
    .unwrap();
    m.append_branch_summary(u.clone(), "tried a rewrite".into(), None, None, false)
        .unwrap();
    m.append_custom_message(
        "review.note",
        serde_json::json!("three findings"),
        true,
        None,
    )
    .unwrap();
    m.append_agent_message(AgentMessage::BashExecution(
        crate::agent_message::BashExecutionMessage {
            command: "git status".into(),
            output: "clean".into(),
            exit_code: Some(0),
            cancelled: false,
            truncated: false,
            full_output_path: None,
            timestamp: 0,
            exclude_from_context: None,
        },
    ))
    .unwrap();

    let raw = m.build_context_raw();
    let roles: Vec<_> = raw
        .iter()
        .map(crate::agent_message::AgentMessage::role)
        .collect();
    use crate::agent_message::MessageRole;
    assert_eq!(
        roles,
        vec![
            MessageRole::CompactionSummary,
            MessageRole::User,
            MessageRole::Assistant,
            MessageRole::BranchSummary,
            MessageRole::Custom,
            MessageRole::BashExecution,
        ],
        "the raw projection keeps every role intact"
    );

    // The LLM view, by contrast, is all `user`/`assistant` — the wrapper prose a UI must never show.
    let llm = m.build_context().messages;
    assert!(
        llm.iter()
            .any(|msg| matches!(msg, Message::User { content, .. }
            if first_text_blocks(content).contains("compacted into the following summary"))),
        "build_context still flattens the compaction into user prose (the LLM boundary)"
    );
    assert!(
        llm.iter()
            .any(|msg| matches!(msg, Message::User { content, .. }
            if first_text_blocks(content).starts_with("Ran `git status`"))),
        "…and the `!` run into `Ran `cmd`` prose"
    );
}

fn first_text_blocks(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}
