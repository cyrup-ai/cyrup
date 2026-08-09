//! Differential-parity tests for the 1:1 gaps closed against Pi (gap-analysis 05-cyrup-session).
//! Each test cites the Pi behavior it pins.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::path::PathBuf;

use cyrup_core::{Content, EntryId, Message};
use cyrup_session::agent_message::{AgentMessage, BashExecutionMessage, CustomRoleMessage};
use cyrup_session::compaction::cutpoint::{find_cut_point, find_valid_cut_points};
use cyrup_session::compaction::files::format_file_operations;
use cyrup_session::compaction::tokens::{estimate_agent_message, TokenCache};
use cyrup_session::context::{
    branch_summary_message, build_context_messages, compaction_summary_message,
    BRANCH_SUMMARY_PREFIX, BRANCH_SUMMARY_SUFFIX, COMPACTION_SUMMARY_PREFIX,
    COMPACTION_SUMMARY_SUFFIX,
};
use cyrup_session::{
    serialize_conversation, Entry, EntryBase, KnownEntry, NewSessionOpts, SessionHeader,
    SessionLayout, SessionManager,
};
use serde_json::{json, Value};

fn user(s: &str) -> Message {
    Message::User { content: vec![Content::text(s)], timestamp: 0 }
}

fn text_of(m: &Message) -> String {
    let blocks = match m {
        Message::User { content, .. } | Message::ToolResult { content, .. } => content,
        Message::Assistant(a) => &a.content,
    };
    blocks
        .iter()
        .find_map(|b| match b {
            Content::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

fn bash(command: &str, output: &str) -> AgentMessage {
    AgentMessage::BashExecution(BashExecutionMessage {
        command: command.into(),
        output: output.into(),
        exit_code: Some(0),
        cancelled: false,
        truncated: false,
        full_output_path: None,
        timestamp: 0,
        exclude_from_context: None,
    })
}

// ---------------------------------------------------------------- gap 1 ------------------------

#[test]
fn gap1_bash_and_custom_role_entries_survive_into_context() {
    // Pi stores bashExecution/custom roles inside type:"message" entries and convertToLlm renders
    // them as user messages (messages.ts:152-168, session-manager.ts:389-399).
    let cwd = PathBuf::from("/proj/gap1");
    let mut m = SessionManager::in_memory(&cwd, NewSessionOpts::default()).unwrap();
    m.append_message(user("hello")).unwrap();
    m.append_agent_message(bash("ls", "a.txt")).unwrap();
    m.append_agent_message(AgentMessage::Custom(CustomRoleMessage {
        custom_type: "ext.note".into(),
        content: json!("from extension"),
        display: true,
        details: None,
        timestamp: 0,
    }))
    .unwrap();

    let ctx = m.build_context();
    // All three contribute (the bash as a rendered user message, the custom unwrapped).
    assert_eq!(ctx.messages.len(), 3, "bash/custom roles are not dropped from context");
    assert!(text_of(&ctx.messages[1]).starts_with("Ran `ls`"));
    assert_eq!(text_of(&ctx.messages[2]), "from extension");
}

#[test]
fn gap1_bash_entry_roundtrips_on_disk() {
    // Closing gap 1 must not regress DI-9: a bash-role message survives load+save.
    let dir = tempfile::tempdir().unwrap();
    let cwd = PathBuf::from("/proj/gap1rt");
    let lay = SessionLayout::new(dir.path().to_path_buf(), cwd.clone());
    let mut m = SessionManager::create(&cwd, &lay, NewSessionOpts::default()).unwrap();
    m.append_agent_message(bash("echo hi", "hi")).unwrap();
    // Force a flush by adding an assistant message.
    m.append_message(Message::Assistant(cyrup_core::AssistantMessage::errored(
        "faux".into(),
        "m",
        Some("faux".into()),
        cyrup_core::StopReason::Stop,
        "",
    )))
    .unwrap();
    let reopened = SessionManager::open(m.session_file().unwrap()).unwrap();
    let has_bash = reopened.entries().iter().any(|e| {
        matches!(e, Entry::Known(KnownEntry::Message { message: AgentMessage::BashExecution(_), .. }))
    });
    assert!(has_bash, "bash-role message round-trips as a typed entry, not Entry::Unknown");
}

// ---------------------------------------------------------------- gap 3 / 4 --------------------

#[test]
fn gap3_4_summary_wrapper_text_is_pi_exact() {
    let c = compaction_summary_message("BODY", 999, 0);
    assert_eq!(text_of(&c), format!("{COMPACTION_SUMMARY_PREFIX}BODY{COMPACTION_SUMMARY_SUFFIX}"));
    // Pi never leaks tokensBefore into the prompt.
    assert!(!text_of(&c).contains("999"));
    assert!(text_of(&c).starts_with("The conversation history before this point was compacted"));

    let b = branch_summary_message("WORK", 0);
    assert_eq!(text_of(&b), format!("{BRANCH_SUMMARY_PREFIX}WORK{BRANCH_SUMMARY_SUFFIX}"));
    assert!(text_of(&b).starts_with("The following is a summary of a branch"));
}

// ---------------------------------------------------------------- gap 5 ------------------------

#[test]
fn gap5_cut_point_validity_excludes_settings_and_summaries() {
    // model_change / thinking_level_change / compaction / custom / label / session_info are NOT
    // valid cut points (compaction.ts:326-334); only message(non-toolResult)/branch_summary/
    // custom_message are.
    fn ent(k: KnownEntry) -> Entry {
        Entry::known(k)
    }
    let base = |id: &str| EntryBase {
        id: EntryId::from(id),
        parent_id: None,
        timestamp: "2026-01-01T00:00:00Z".into(),
    };
    let entries = vec![
        ent(KnownEntry::Message { base: base("e0"), message: AgentMessage::Core(user("hi")) }),
        ent(KnownEntry::ModelChange {
            base: base("e1"),
            provider: "p".into(),
            model_id: "m".into(),
        }),
        ent(KnownEntry::SessionInfo { base: base("e2"), name: Some("n".into()) }),
        ent(KnownEntry::CustomMessage {
            base: base("e3"),
            custom_type: "x".into(),
            content: json!("c"),
            display: true,
            details: None,
        }),
    ];
    let valid = find_valid_cut_points(&entries, 0, entries.len());
    // Only the message (0) and the custom_message (3) are valid; model_change/session_info are not.
    assert_eq!(valid, vec![0, 3]);
}

// ---------------------------------------------------------------- gap 6 ------------------------

#[test]
fn gap6_back_scan_folds_leading_non_message_entries() {
    // After choosing a cut, leading non-message entries (e.g. model_change) are folded into the
    // kept region, stopping at a message (compaction.ts:429-442).
    fn ent(k: KnownEntry) -> Entry {
        Entry::known(k)
    }
    let base = |id: &str, p: Option<&str>| EntryBase {
        id: EntryId::from(id),
        parent_id: p.map(EntryId::from),
        timestamp: "2026-01-01T00:00:00Z".into(),
    };
    let big = "word ".repeat(80);
    let entries = vec![
        ent(KnownEntry::Message {
            base: base("e0", None),
            message: AgentMessage::Core(user(&big)),
        }),
        ent(KnownEntry::ModelChange {
            base: base("e1", Some("e0")),
            provider: "p".into(),
            model_id: "m".into(),
        }),
        ent(KnownEntry::Message {
            base: base("e2", Some("e1")),
            message: AgentMessage::Core(user("recent enough words here to matter")),
        }),
    ];
    let cache = TokenCache::default();
    // keep-recent small so the budget walk lands at e2; the back-scan then folds the leading
    // model_change (e1) INTO the kept region (cutIndex moves from 2 to 1), stopping at e0 (a
    // message). So the leading settings change travels with the recent messages, not into history.
    let cut = find_cut_point(&entries, &cache, 0, entries.len(), 5);
    assert_eq!(cut.first_kept_index, 1, "back-scan folded the model_change into the kept region");
    assert!(matches!(
        entries.get(cut.first_kept_index),
        Some(Entry::Known(KnownEntry::ModelChange { .. }))
    ));
}

// ---------------------------------------------------------------- gap 1 token estimate ---------

#[test]
fn bash_token_estimate_matches_pi_raw_rule() {
    // Pi estimateTokens(bashExecution) = (command.length + output.length)/4 (compaction.ts:284-287),
    // NOT the rendered text length.
    let b = bash("abcd", "efgh"); // 4 + 4 = 8 chars → 2 tokens
    assert_eq!(estimate_agent_message(&b), 2);
}

// ---------------------------------------------------------------- gap 12 -----------------------

#[test]
fn gap12_session_id_validation() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = PathBuf::from("/proj/gap12");
    let lay = SessionLayout::new(dir.path().to_path_buf(), cwd.clone());
    // Invalid ids are rejected (assertValidSessionId, session-manager.ts:207-213).
    for bad in ["bad/id", "", "-leading", "trailing-", "has space"] {
        let opts = NewSessionOpts { id: Some(bad.into()), parent_session: None };
        assert!(
            SessionManager::create(&cwd, &lay, opts).is_err(),
            "id {bad:?} must be rejected"
        );
    }
    // A valid id is accepted.
    let opts = NewSessionOpts { id: Some("good.id-1_OK".into()), parent_session: None };
    assert!(SessionManager::create(&cwd, &lay, opts).is_ok());
}

// ---------------------------------------------------------------- gap 13 -----------------------

#[test]
fn gap13_tree_promotes_orphans_and_self_parent_to_roots() {
    // getTree treats self-parent and missing-parent entries as roots (session-manager.ts:1212-1222);
    // a self-parent must not recurse forever.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("2026-01-01_sess.jsonl");
    let header = SessionHeader::new("sess".into(), "/proj/gap13", "2026-01-01T00:00:00Z");
    let mut text = serde_json::to_string(&header).unwrap();
    text.push('\n');
    // e0: normal root. e1: self-parent. e2: orphan (parent "ghost" not present).
    let lines = [
        json!({"type":"message","id":"e0","parentId":null,"timestamp":"2026-01-01T00:00:01Z","message":{"role":"user","content":"a","timestamp":0}}),
        json!({"type":"message","id":"e1","parentId":"e1","timestamp":"2026-01-01T00:00:02Z","message":{"role":"user","content":"b","timestamp":0}}),
        json!({"type":"message","id":"e2","parentId":"ghost","timestamp":"2026-01-01T00:00:03Z","message":{"role":"user","content":"c","timestamp":0}}),
    ];
    for l in lines {
        text.push_str(&l.to_string());
        text.push('\n');
    }
    std::fs::write(&file, text).unwrap();

    let m = SessionManager::open(&file).unwrap();
    let tree = m.tree(); // must terminate (no infinite recursion on the self-parent)
    let root_ids: Vec<String> =
        tree.iter().map(|n| n.entry.id().as_str().to_string()).collect();
    assert!(root_ids.contains(&"e0".to_string()));
    assert!(root_ids.contains(&"e1".to_string()), "self-parent promoted to root");
    assert!(root_ids.contains(&"e2".to_string()), "orphan promoted to root, never dropped");
}

// ---------------------------------------------------------------- gap 14 / 15 ------------------

#[test]
fn gap14_15_listing_first_message_user_only_with_sentinel() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = PathBuf::from("/proj/gap1415");
    let lay = SessionLayout::new(dir.path().to_path_buf(), cwd.clone());

    // Session A: assistant first, then user — firstMessage must be the USER text, not the assistant.
    let mut a = SessionManager::create(&cwd, &lay, NewSessionOpts::default()).unwrap();
    a.append_message(Message::Assistant(cyrup_core::AssistantMessage::errored(
        "faux".into(),
        "m",
        Some("faux".into()),
        cyrup_core::StopReason::Stop,
        "",
    )))
    .unwrap();
    a.append_message(Message::Assistant(assistant_text("assistant speaks first")))
        .unwrap();
    a.append_message(user("the user question")).unwrap();
    a.append_message(Message::ToolResult {
        tool_call_id: "t".into(),
        tool_name: "n".into(),
        content: vec![Content::text("TOOL-OUTPUT-SHOULD-NOT-APPEAR")],
        is_error: false,
        details: None,
        timestamp: 0,
        usage: None,
        added_tool_names: Vec::new(),
    })
    .unwrap();

    let infos = cyrup_session::list(&lay);
    let info = infos.iter().find(|i| i.id.as_str() == a.session_id().as_str()).unwrap();
    assert_eq!(info.first_message, "the user question", "firstMessage is the first USER message");
    assert!(
        !info.all_messages_text.contains("TOOL-OUTPUT-SHOULD-NOT-APPEAR"),
        "allMessagesText excludes toolResult text"
    );
    assert!(info.all_messages_text.contains("assistant speaks first"));
    // messageCount counts every message entry (incl. assistant + toolResult).
    assert_eq!(info.message_count, 4);
}

fn assistant_text(s: &str) -> cyrup_core::AssistantMessage {
    cyrup_core::AssistantMessage {
        content: vec![Content::text(s)],
        provider: "faux".into(),
        model: "m".into(),
        api: "faux".into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: cyrup_core::Usage::default(),
        stop_reason: cyrup_core::StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        timestamp: 0,
    }
}

// ---------------------------------------------------------------- gap 16 -----------------------

#[test]
fn gap16_migration_renames_hookmessage_to_custom() {
    // v2→v3 renames message.role "hookMessage" → "custom" so it stays in context
    // (session-manager.ts:255-270). After rename it parses as the custom AgentMessage arm.
    let mut header = SessionHeader::new("s".into(), "/proj/gap16", "2026-01-01T00:00:00Z");
    header.version = Some(2);
    let raw = json!({
        "type": "message",
        "id": "e0",
        "parentId": null,
        "timestamp": "2026-01-01T00:00:01Z",
        "message": {
            "role": "hookMessage",
            "customType": "legacy.hook",
            "content": "legacy content",
            "display": true,
            "timestamp": 0
        }
    });
    let mut entries = vec![serde_json::from_value::<Entry>(raw).unwrap()];
    // Before migration the hookMessage role is unrepresentable → held as Unknown.
    assert!(matches!(entries[0], Entry::Unknown(_)));

    let changed = cyrup_session::migrate::to_current(&mut header, &mut entries);
    assert!(changed);
    assert_eq!(header.version, Some(3));
    // Now it is a typed custom-role message and contributes to context.
    match &entries[0] {
        Entry::Known(KnownEntry::Message { message: AgentMessage::Custom(c), .. }) => {
            assert_eq!(c.custom_type, "legacy.hook");
        }
        other => panic!("expected custom-role message, got {other:?}"),
    }
    let refs: Vec<&Entry> = entries.iter().collect();
    let msgs = build_context_messages(&refs);
    assert_eq!(text_of(&msgs[0]), "legacy content");
}

// ---------------------------------------------------------------- gap 17 -----------------------

#[test]
fn gap17_serialize_separators_json_args_and_skips_empty() {
    use cyrup_core::{AssistantMessage, StopReason, ToolCall, Usage};
    let asst = Message::Assistant(AssistantMessage {
        content: vec![
            Content::ToolCall(ToolCall {
                id: "t1".into(),
                name: "read".into(),
                arguments: json!({ "path": "a.rs" }).as_object().cloned().unwrap(),
                thought_signature: None,
            }),
            Content::ToolCall(ToolCall {
                id: "t2".into(),
                name: "write".into(),
                arguments: json!({ "path": "b.rs" }).as_object().cloned().unwrap(),
                thought_signature: None,
            }),
        ],
        provider: "faux".into(),
        model: "m".into(),
        api: "faux".into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::ToolUse,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        timestamp: 0,
    });
    // An empty user message must NOT emit a "[User]: " line.
    let empty_user = Message::User { content: vec![Content::text("")], timestamp: 0 };
    let out = serialize_conversation(&[empty_user, asst]);
    assert!(!out.contains("[User]:"), "empty user line skipped");
    // Calls joined with "; ", args JSON-encoded (string quoted).
    assert!(out.contains("read(path=\"a.rs\"); write(path=\"b.rs\")"), "got: {out}");
}

// ---------------------------------------------------------------- gap 18 -----------------------

#[test]
fn gap18_format_file_operations_only_non_empty_sections() {
    // Only the read section when there are no modified files (utils.ts:72-82).
    let only_read = format_file_operations(&["a.rs".into()], &[]);
    assert!(only_read.contains("<read-files>"));
    assert!(!only_read.contains("<modified-files>"), "no empty modified section");
    let only_mod = format_file_operations(&[], &["b.rs".into()]);
    assert!(!only_mod.contains("<read-files>"), "no empty read section");
    assert!(only_mod.contains("<modified-files>"));
    assert_eq!(format_file_operations(&[], &[]), "");
}

// ---------------------------------------------------------------- gap 19 / 20 ------------------

#[test]
fn gap19_20_prompts_are_pi_verbatim() {
    use cyrup_session::compaction::summarize::{
        SUMMARIZATION_PROMPT, SUMMARIZATION_SYSTEM_PROMPT, TURN_PREFIX_SUMMARIZATION_PROMPT,
        UPDATE_SUMMARIZATION_PROMPT,
    };
    use cyrup_session::compaction::branch::{BRANCH_SUMMARY_PREAMBLE, BRANCH_SUMMARY_PROMPT};

    assert!(SUMMARIZATION_SYSTEM_PROMPT
        .starts_with("You are a context summarization assistant."));
    assert!(SUMMARIZATION_PROMPT.contains(
        "The messages above are a conversation to summarize."
    ));
    assert!(SUMMARIZATION_PROMPT.contains("## Critical Context"));
    assert!(UPDATE_SUMMARIZATION_PROMPT.contains(
        "The messages above are NEW conversation messages to incorporate"
    ));
    assert!(TURN_PREFIX_SUMMARIZATION_PROMPT
        .starts_with("This is the PREFIX of a turn that was too large to keep."));
    // The branch prompt has NO Critical Context section (unlike compaction).
    assert!(BRANCH_SUMMARY_PROMPT
        .starts_with("Create a structured summary of this conversation branch"));
    assert!(!BRANCH_SUMMARY_PROMPT.contains("## Critical Context"));
    assert!(BRANCH_SUMMARY_PREAMBLE
        .starts_with("The user explored a different conversation branch before returning here."));
}

// ============================================================== round-2 gaps ==================
// Lifecycle/listing divergences re-derived in 05-cyrup-session.md (#12, #21, #22, #23, #24).

use cyrup_core::SessionId;
use cyrup_session::{
    list_all_in_dir, list_in_dir, newest_session, DiskStore, SessionError, SessionStore,
};

fn asst() -> Message {
    Message::Assistant(cyrup_core::AssistantMessage::errored(
        "faux".into(),
        "m",
        Some("faux".into()),
        cyrup_core::StopReason::Stop,
        "",
    ))
}

/// Write a minimal valid session file (header line only) with an explicit header cwd.
fn write_header_only(dir: &std::path::Path, fname: &str, id: &str, cwd: &str) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(fname);
    let header = SessionHeader::new(SessionId::from(id), cwd, "2026-01-01T00:00:00Z");
    let line = serde_json::to_string(&header).unwrap();
    std::fs::write(&path, format!("{line}\n")).unwrap();
    path
}

// ---------------------------------------------------------------- gap 12 -----------------------

#[test]
fn gap12_in_memory_validates_caller_id() {
    // Pi inMemory routes through the constructor's assertValidSessionId
    // (session-manager.ts:830-831,1437-1439): a malformed id is rejected even for an ephemeral
    // session, exactly as for a persisted one. cyrup's `in_memory` adopts the validating signature
    // 1:1 (returns `Result`).
    let cwd = PathBuf::from("/proj/gap12");

    // A bad id (leading dot / illegal char) is rejected.
    let bad = NewSessionOpts { id: Some(SessionId::from(".bad/id")), parent_session: None };
    match SessionManager::in_memory(&cwd, bad) {
        Err(SessionError::InvalidSessionId(_)) => {}
        Err(e) => panic!("wrong error: {e:?}"),
        Ok(_) => panic!("malformed id should be rejected"),
    }

    // A valid id is accepted and preserved verbatim.
    let good = NewSessionOpts { id: Some(SessionId::from("good-1.0_x")), parent_session: None };
    let m = SessionManager::in_memory(&cwd, good).unwrap();
    assert_eq!(m.session_id().as_str(), "good-1.0_x");

    // None → a fresh id is generated (no error).
    assert!(SessionManager::in_memory(&cwd, NewSessionOpts::default()).is_ok());
}

// ---------------------------------------------------------------- gap 21 -----------------------

#[test]
fn gap21_list_reports_progress_loaded_and_total() {
    // Pi list/listAll invoke onProgress(loaded, total) after each file with a stable total
    // (session-manager.ts:1507-1516,731-734). cyrup's list_in_dir mirrors the (loaded, total)
    // affordance.
    let dir = tempfile::tempdir().unwrap();
    let cwd = PathBuf::from("/proj/gap21");
    let lay = SessionLayout::new(dir.path().to_path_buf(), cwd.clone());
    let sdir = lay.dir();
    write_header_only(&sdir, "2026-01-01T00-00-00_a.jsonl", "a", "/proj/gap21");
    write_header_only(&sdir, "2026-01-01T00-00-01_b.jsonl", "b", "/proj/gap21");
    write_header_only(&sdir, "2026-01-01T00-00-02_c.jsonl", "c", "/proj/gap21");

    let mut seen: Vec<(usize, usize)> = Vec::new();
    let mut cb = |loaded: usize, total: usize| seen.push((loaded, total));
    let infos = list_in_dir(&sdir, None, Some(&mut cb));

    assert_eq!(infos.len(), 3);
    assert_eq!(seen.len(), 3, "progress fires once per file");
    assert!(seen.iter().all(|&(_, total)| total == 3), "total is stable at the file count");
    assert_eq!(seen.last().copied(), Some((3, 3)), "loaded reaches total");
    // loaded is monotonic 1..=total.
    assert_eq!(seen.iter().map(|&(l, _)| l).collect::<Vec<_>>(), vec![1, 2, 3]);
}

// ---------------------------------------------------------------- gap 22 -----------------------

#[test]
fn gap22_list_in_dir_filters_by_cwd_for_shared_directory() {
    // Pi filters list results to the requesting cwd when a custom/shared sessionDir is used
    // (filterCwd + sessionCwdMatches, session-manager.ts:534-536,1509-1513), so a directory holding
    // sessions from several projects only shows the current one.
    let dir = tempfile::tempdir().unwrap();
    let shared = dir.path().join("shared");
    write_header_only(&shared, "s_mine.jsonl", "mine", "/proj/here");
    write_header_only(&shared, "s_other.jsonl", "other", "/proj/elsewhere");

    // No filter → both projects' sessions.
    let all = list_in_dir(&shared, None, None);
    assert_eq!(all.len(), 2, "unfiltered listing returns every session in the dir");

    // cwd filter → only sessions whose header cwd matches.
    let here = std::path::Path::new("/proj/here");
    let mine = list_in_dir(&shared, Some(here), None);
    assert_eq!(mine.len(), 1, "shared dir filtered to the current cwd");
    assert_eq!(mine[0].cwd, "/proj/here");

    // The listAll(sessionDir) overload lists a custom dir with no cwd filter.
    assert_eq!(list_all_in_dir(&shared, None).len(), 2);
}

#[test]
fn gap22_continue_recent_filtered_skips_other_projects() {
    // Pi continueRecent applies the cwd filter for a shared dir (session-manager.ts:1426-1434):
    // resume the newest session FOR THIS cwd, not the newest session overall.
    let dir = tempfile::tempdir().unwrap();
    let cwd = PathBuf::from("/proj/mine");
    let lay = SessionLayout::new(dir.path().to_path_buf(), cwd.clone());
    let sdir = lay.dir();
    // `mine` written first, `other` (a different project) written last → newest by mtime.
    write_header_only(&sdir, "2026-01-01T00-00-00_mine.jsonl", "mine", "/proj/mine");
    write_header_only(&sdir, "2026-01-01T00-00-09_other.jsonl", "other", "/proj/elsewhere");

    // newest_session with a cwd filter skips the newer foreign session.
    let picked = newest_session(&sdir, Some(cwd.as_path())).unwrap();
    assert!(picked.to_string_lossy().contains("mine"), "filtered newest = current project");

    // continue_recent_filtered resumes that one (its header cwd is /proj/mine).
    let resumed = SessionManager::continue_recent_filtered(&cwd, &lay, true).unwrap();
    assert_eq!(resumed.header().cwd, "/proj/mine");
    assert_eq!(resumed.session_id().as_str(), "mine");
}

// ---------------------------------------------------------------- gap 23 -----------------------

#[test]
fn gap23_create_exclusive_refuses_to_clobber() {
    // Pi's first-flush openSync(file,"wx") / forkFrom writeFileSync {flag:"wx"} throw EEXIST rather
    // than overwrite a pre-existing file (session-manager.ts:927,1489) — the duplicate-header guard.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("excl.jsonl");
    let header = SessionHeader::new(SessionId::from("e1"), "/c", "2026-01-01T00:00:00Z");
    let mut store = DiskStore::new(&path);

    store.create_exclusive(&header, &[]).unwrap();
    assert!(path.exists());

    // A second exclusive-create on the same path fails (does not clobber).
    let err = store.create_exclusive(&header, &[]).unwrap_err();
    assert!(matches!(err, SessionError::AlreadyExists(_)), "got: {err:?}");
}

#[test]
fn gap23_first_flush_uses_exclusive_create() {
    // The deferred first flush goes through create_exclusive: no file until an assistant exists,
    // then the whole buffer lands (session-manager.ts:926-935).
    let dir = tempfile::tempdir().unwrap();
    let cwd = PathBuf::from("/proj/gap23");
    let lay = SessionLayout::new(dir.path().to_path_buf(), cwd.clone());
    let mut m = SessionManager::create(&cwd, &lay, NewSessionOpts::default()).unwrap();
    m.append_message(user("hi")).unwrap();
    assert!(!m.session_file().unwrap().exists(), "no file before the first assistant message");
    m.append_message(asst()).unwrap();
    assert!(m.session_file().unwrap().exists(), "file created on first assistant message");
}

// ---------------------------------------------------------------- gap 24 -----------------------

#[test]
fn gap24_clone_defers_write_until_assistant_exists() {
    // Pi createBranchedSession defers the file write until an assistant message exists
    // (session-manager.ts:1362-1368), avoiding an empty branched file + the duplicate-header bug.
    let dir = tempfile::tempdir().unwrap();
    let cwd = PathBuf::from("/proj/gap24");
    let lay = SessionLayout::new(dir.path().to_path_buf(), cwd.clone());
    let mut m = SessionManager::create(&cwd, &lay, NewSessionOpts::default()).unwrap();
    m.append_message(user("just a question")).unwrap(); // no assistant yet

    // Branch the assistant-less path in place: the new session must NOT have written a file.
    let leaf = m.leaf_id().cloned().unwrap();
    let cloned_path = m
        .create_branched_session(&leaf, &lay)
        .unwrap()
        .expect("persisted branch returns a path");
    assert!(!cloned_path.exists(), "branched session with no assistant defers its file");

    // Once an assistant message arrives, the deferred buffer is flushed via create_exclusive.
    m.append_message(asst()).unwrap();
    assert!(cloned_path.exists(), "deferred branched file is created on the first assistant message");
    // The retained user message survived into the new file.
    let reopened = SessionManager::open(&cloned_path).unwrap();
    assert!(reopened.entries().iter().any(|e| matches!(
        e,
        Entry::Known(KnownEntry::Message { message: AgentMessage::Core(Message::User { .. }), .. })
    )));
}

#[test]
fn gap24_clone_with_assistant_writes_eagerly() {
    // When the cloned path already contains an assistant message, Pi writes immediately
    // (session-manager.ts:1363-1365); cyrup mirrors this (eager rewrite, flushed=true).
    let dir = tempfile::tempdir().unwrap();
    let cwd = PathBuf::from("/proj/gap24b");
    let lay = SessionLayout::new(dir.path().to_path_buf(), cwd.clone());
    let mut m = SessionManager::create(&cwd, &lay, NewSessionOpts::default()).unwrap();
    m.append_message(user("q")).unwrap();
    m.append_message(asst()).unwrap();

    let leaf = m.leaf_id().cloned().unwrap();
    let cloned_path = m
        .create_branched_session(&leaf, &lay)
        .unwrap()
        .expect("persisted branch returns a path");
    assert!(cloned_path.exists(), "assistant-bearing clone is written eagerly");
}

// ---------------------------------------------------------------- M3 / M4 branched labels -----

/// M3 + M4: `createBranchedSession` re-emits retained-target labels using the ORIGINAL timestamp
/// from `labelTimestampsById` and collects them from the GLOBAL `labelsById` map for any target in
/// the retained path (`session-manager.ts:1324-1331,1338-1343`) — NOT `now()`, and NOT only the
/// `Label` entries that happen to lie on the branched path.
///
/// Fixture (constructed from Pi source semantics): three label entries hang OFF the branched path
/// (they are children of the leaf / each other), so under the OLD on-path scan they would all be
/// dropped:
///   - L1 targets the on-path user `e1`, label "important", original ts 2020-01-01 (must survive)
///   - L2 targets the on-path assistant `e2`, label "temp", ts 2021-01-01
///   - L3 clears `e2` (label:null), ts 2022-01-01  → e2 must NOT be re-emitted
///
/// Pi's `labelsById` (== cyrup's `self.labels`) ends as `{e1 -> ("important", 2020)}`.
#[test]
fn m3_m4_branched_labels_keep_original_ts_and_global_scope() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("2026-01-01T00-00-00-000Z_aaaaaaaa.jsonl");
    let contents = concat!(
        r#"{"type":"session","version":3,"id":"11111111-1111-7111-8111-111111111111","timestamp":"2026-01-01T00:00:00Z","cwd":"/proj/m3"}"#, "\n",
        r#"{"type":"message","id":"e1","parentId":null,"timestamp":"2026-01-01T00:00:01Z","message":{"role":"user","content":[{"type":"text","text":"hi"}],"timestamp":0}}"#, "\n",
        r#"{"type":"message","id":"e2","parentId":"e1","timestamp":"2026-01-01T00:00:02Z","message":{"role":"assistant","content":[{"type":"text","text":"reply"}],"provider":"faux","model":"f","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}},"stopReason":"stop","timestamp":0}}"#, "\n",
        // OFF-PATH labels (children of e2 / each other), not on the root->e2 path:
        r#"{"type":"label","id":"L1","parentId":"e2","timestamp":"2020-01-01T00:00:00.000Z","targetId":"e1","label":"important"}"#, "\n",
        r#"{"type":"label","id":"L2","parentId":"L1","timestamp":"2021-01-01T00:00:00.000Z","targetId":"e2","label":"temp"}"#, "\n",
        r#"{"type":"label","id":"L3","parentId":"L2","timestamp":"2022-01-01T00:00:00.000Z","targetId":"e2"}"#, "\n",
    );
    std::fs::write(&file, contents).unwrap();

    let mut m = SessionManager::open(&file).unwrap();
    // Sanity: global label map resolves the off-path label, and e2 was cleared.
    assert_eq!(m.label(&EntryId::from("e1")), Some("important"));
    assert_eq!(m.label(&EntryId::from("e2")), None);

    // Branch onto the root->e2 path. The three label entries are OFF this path.
    let lay = SessionLayout::new(dir.path().to_path_buf(), PathBuf::from("/proj/m3"));
    m.create_branched_session(&EntryId::from("e2"), &lay).unwrap();

    // Collect the re-attached label entries in the new (in-place) session.
    let labels: Vec<(&str, &str)> = m
        .entries()
        .iter()
        .filter_map(|e| match e {
            Entry::Known(KnownEntry::Label { base, target_id, label: Some(_), .. }) => {
                Some((target_id.as_str(), base.timestamp.as_str()))
            }
            _ => None,
        })
        .collect();

    // M4 (global scope): the off-path label targeting on-path e1 SURVIVED (old on-path scan dropped it).
    // M4 (cleared): e2's set-then-cleared label was NOT re-emitted.
    // M3 (timestamp): it carries the ORIGINAL 2020 timestamp, not now().
    assert_eq!(
        labels,
        vec![("e1", "2020-01-01T00:00:00.000Z")],
        "exactly the live e1 label, re-emitted with its ORIGINAL timestamp"
    );
    // And the label value is preserved.
    assert_eq!(m.label(&EntryId::from("e1")), Some("important"));
}

// ================================================================================================
// Round 3 — residual 1:1 gaps (G-1..G-7)
// ================================================================================================

fn ts_of(m: &Message) -> i64 {
    match m {
        Message::User { timestamp, .. } | Message::ToolResult { timestamp, .. } => *timestamp,
        Message::Assistant(a) => a.timestamp,
    }
}

// ---------------------------------------------------------------- G-1 token ceil ---------------

#[test]
fn g1_token_estimate_rounds_up_like_pi_ceil() {
    // Pi estimateTokens returns Math.ceil(chars / 4) in EVERY arm (compaction.ts:264,277,287,291).
    // A floor would under-count and shift the cut-point / trigger. bash("abc","de") = 5 chars.
    let b = bash("abc", "de"); // 3 + 2 = 5 chars → ceil(5/4) = 2 (floor would give 1)
    assert_eq!(estimate_agent_message(&b), 2, "bashExecution estimate rounds up");

    // Core user message: 9 chars → ceil(9/4) = 3 (floor would give 2).
    let u = AgentMessage::Core(user("123456789"));
    assert_eq!(estimate_agent_message(&u), 3, "core message estimate rounds up");

    // Exact multiple of 4 is unaffected by the rounding mode.
    let exact = bash("abcd", "efgh"); // 8 chars → 2
    assert_eq!(estimate_agent_message(&exact), 2);
}

// ---------------------------------------------------------------- G-2 v1→v2 migration ----------

#[test]
fn g2_v1_to_v2_converts_first_kept_entry_index_to_id() {
    use cyrup_session::migrate::to_current;

    // A legacy v1 file: no ids, a compaction referencing its first-kept entry by NUMERIC index.
    // Pi migrateV1ToV2 (session-manager.ts:241-250) resolves that index (over the header-inclusive
    // array) into firstKeptEntryId and drops the index, otherwise the compaction can never parse.
    let mut header = SessionHeader::new(
        cyrup_core::SessionId::from("sess-v1"),
        "/proj",
        "2026-01-01T00:00:00Z",
    );
    header.version = None; // v1 (no version field)

    let msg = |t: &str, body: &str| {
        let m = serde_json::to_value(AgentMessage::Core(user(body))).unwrap();
        serde_json::from_value::<Entry>(json!({
            "type": "message",
            "timestamp": t,
            "message": m,
        }))
        .unwrap()
    };
    // file indices (header-inclusive): 0=header, 1=entries[0], 2=entries[1], 3=entries[2].
    let mut entries = vec![
        msg("2026-01-01T00:00:01Z", "first"),
        msg("2026-01-01T00:00:02Z", "kept"),
        serde_json::from_value::<Entry>(json!({
            "type": "compaction",
            "timestamp": "2026-01-01T00:00:03Z",
            "summary": "SUM",
            "tokensBefore": 42,
            "firstKeptEntryIndex": 2, // → cyrup pos 1 (entries[1], "kept")
        }))
        .unwrap(),
    ];
    // Pre-migration the compaction cannot parse (no id, no firstKeptEntryId).
    assert!(matches!(entries[2], Entry::Unknown(_)), "v1 compaction starts as Unknown");

    assert!(to_current(&mut header, &mut entries));
    assert_eq!(header.version, Some(cyrup_session::CURRENT_VERSION));

    let kept_id = entries[1].id();
    match &entries[2] {
        Entry::Known(KnownEntry::Compaction { first_kept_entry_id, summary, tokens_before, .. }) => {
            assert_eq!(
                first_kept_entry_id.as_ref(),
                Some(&kept_id),
                "index resolved to the kept entry's id"
            );
            assert_eq!(summary, "SUM");
            assert_eq!(*tokens_before, 42);
        }
        other => panic!("compaction should now parse: {other:?}"),
    }
    // The numeric index field is gone from the re-typed entry.
    let line = entries[2].to_line().unwrap();
    assert!(!line.contains("firstKeptEntryIndex"), "index field removed: {line}");
}

// ---------------------------------------------------------------- G-3 file-op extraction -------

fn asst_toolcall(name: &str, key: &str, path: &str) -> Message {
    use cyrup_core::{AssistantMessage, StopReason, ToolCall, Usage};
    Message::Assistant(AssistantMessage {
        content: vec![Content::ToolCall(ToolCall {
            id: "tc".into(),
            name: name.into(),
            arguments: json!({ key: path }).as_object().cloned().unwrap(),
            thought_signature: None,
        })],
        provider: "faux".into(),
        model: "m".into(),
        api: "faux".into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::ToolUse,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        timestamp: 0,
    })
}

#[test]
fn g3_file_ops_match_exact_tool_name_and_path_arg_only() {
    use cyrup_session::compaction::files::FileOps;
    // Pi extractFileOpsFromMessage (utils.ts:38-55): EXACT tool name switch + ONLY args.path.
    let mut ops = FileOps::default();
    ops.absorb_message(&asst_toolcall("read", "path", "a.rs")); // tracked (read)
    ops.absorb_message(&asst_toolcall("edit", "path", "b.rs")); // tracked (modified)
    ops.absorb_message(&asst_toolcall("multiedit", "path", "z.rs")); // NOT an exact name → skipped
    ops.absorb_message(&asst_toolcall("read", "file_path", "y.rs")); // path under wrong key → skipped

    let (read, modified) = ops.compute_lists();
    assert_eq!(read, vec!["a.rs".to_string()], "only exact 'read' with args.path tracked");
    assert_eq!(modified, vec!["b.rs".to_string()], "only exact 'edit' with args.path tracked");
}

// ---------------------------------------------------------------- G-4 name sanitization --------

#[test]
fn g4_session_name_is_sanitized_on_write_and_trimmed_on_read() {
    // Pi appendSessionInfo: name.replace(/[\r\n]+/g," ").trim() (session-manager.ts:1031);
    // getSessionName trims and maps empty → undefined (session-manager.ts:1052).
    let cwd = PathBuf::from("/proj/g4");
    let mut m = SessionManager::in_memory(&cwd, NewSessionOpts::default()).unwrap();
    m.append_session_info("  Hello\r\n\nWorld  ").unwrap();
    assert_eq!(m.session_name().as_deref(), Some("Hello World"), "newline run collapsed + trimmed");

    // The persisted entry stores the sanitized bytes (no raw newline in the JSONL line).
    let line = m
        .entries()
        .iter()
        .rev()
        .find_map(|e| match e {
            Entry::Known(KnownEntry::SessionInfo { .. }) => e.to_line().ok(),
            _ => None,
        })
        .unwrap();
    assert!(!line.contains('\n') && !line.contains('\r'), "no raw newline persisted: {line}");

    // A later whitespace-only name clears the title (empty → None).
    m.append_session_info("   \n  ").unwrap();
    assert_eq!(m.session_name(), None, "empty name clears the session title");
}

// ---------------------------------------------------------------- G-5 summary timestamps -------

#[test]
fn g5_summary_messages_carry_entry_timestamp() {
    // Pi createCompactionSummaryMessage / createBranchSummaryMessage set
    // timestamp: new Date(entry.timestamp).getTime() (messages.ts:100-120).
    assert_eq!(ts_of(&compaction_summary_message("S", 0, 777)), 777);
    assert_eq!(ts_of(&branch_summary_message("S", 555)), 555);

    // And the timestamp flows through build_context_messages from the compaction entry.
    let base = |id: &str, p: Option<&str>, t: &str| EntryBase {
        id: EntryId::from(id),
        parent_id: p.map(EntryId::from),
        timestamp: t.into(),
    };
    let entries = [
        Entry::known(KnownEntry::Message {
            base: base("e0", None, "2026-01-01T00:00:00Z"),
            message: AgentMessage::Core(user("dropped pre-compaction")),
        }),
        Entry::known(KnownEntry::Compaction {
            base: base("e1", Some("e0"), "2026-01-01T00:00:01Z"),
            summary: "SUM".into(),
            first_kept_entry_id: Some(EntryId::from("e0")),
            tokens_before: 9,
            details: None,
            usage: None,
            from_hook: None,
        }),
    ];
    let refs: Vec<&Entry> = entries.iter().collect();
    let msgs = build_context_messages(&refs);
    // Expected ms = 2026-01-01T00:00:01Z.
    let expected = 1767225601000i64;
    assert_eq!(ts_of(&msgs[0]), expected, "compaction summary carries the entry timestamp");
}

// ---------------------------------------------------------------- G-6 tree root order ----------

#[test]
fn g6_tree_leaves_roots_in_insertion_order() {
    // Pi getTree sorts only each node's children; roots stay in entry insertion order
    // (session-manager.ts:1210-1234). Two self-parent roots inserted newest-first must NOT be
    // re-sorted by timestamp.
    let cwd = PathBuf::from("/proj/g6");
    let mut m = SessionManager::in_memory(&cwd, NewSessionOpts::default()).unwrap();
    // First root (later timestamp), then reset and add a second root (earlier-looking).
    m.append_message(user("root A inserted first")).unwrap();
    m.reset_leaf();
    m.append_message(user("root B inserted second")).unwrap();

    let tree = m.tree();
    assert_eq!(tree.len(), 2, "two roots");
    // Insertion order preserved: A before B, regardless of timestamps.
    assert_eq!(text_of_entry(&tree[0].entry), "root A inserted first");
    assert_eq!(text_of_entry(&tree[1].entry), "root B inserted second");
}

fn text_of_entry(e: &Entry) -> String {
    match e {
        Entry::Known(KnownEntry::Message { message: AgentMessage::Core(m), .. }) => text_of(m),
        _ => String::new(),
    }
}

// ---------------------------------------------------------------- G-7 in-memory clone ----------

#[test]
fn g7_clone_of_in_memory_session_writes_no_file() {
    // Pi createBranchedSession(persist:false) clones without touching disk
    // (session-manager.ts:1292-1392). Cloning an in-memory session must stay in memory.
    let dir = tempfile::tempdir().unwrap();
    let cwd = PathBuf::from("/proj/g7");
    let lay = SessionLayout::new(dir.path().to_path_buf(), cwd.clone());
    let mut m = SessionManager::in_memory(&cwd, NewSessionOpts::default()).unwrap();
    m.append_message(user("q")).unwrap();
    m.append_message(asst()).unwrap(); // assistant present → would force eager write on disk

    let leaf = m.leaf_id().cloned().unwrap();
    let before = std::fs::read_dir(dir.path()).unwrap().count();
    let path = m.create_branched_session(&leaf, &lay).unwrap();
    let after = std::fs::read_dir(dir.path()).unwrap().count();

    assert!(path.is_none(), "in-memory branch returns no path (Pi returns undefined)");
    assert!(!m.is_persisted(), "in-memory clone stays in memory");
    assert!(m.session_file().is_none(), "no backing file path");
    assert_eq!(before, after, "no file created on disk for an in-memory clone");
    // The retained path still carries the messages in memory.
    assert!(!m.entries().is_empty(), "retained entries are present in memory");
}

// ---------------------------------------------------------------- G-2 explicit-leaf branch -----

#[test]
fn g2_create_branched_session_explicit_leaf_in_place() {
    // Pi createBranchedSession(leafId) takes an EXPLICIT leaf and mutates the manager IN PLACE
    // (session-manager.ts:1292-1392): the manager re-roots onto root→leafId only, the previous file
    // is untouched, and parentSession records it. Here we branch at an earlier (non-leaf) entry.
    let dir = tempfile::tempdir().unwrap();
    let cwd = PathBuf::from("/proj/g2");
    let lay = SessionLayout::new(dir.path().to_path_buf(), cwd.clone());
    let mut m = SessionManager::create(&cwd, &lay, NewSessionOpts::default()).unwrap();
    m.append_message(user("q0")).unwrap();
    let a0 = m.append_message(asst()).unwrap();
    m.append_message(user("q1")).unwrap();
    m.append_message(asst()).unwrap();
    let old_file = m.session_file().unwrap().to_path_buf();
    let old_count = m.entries().len(); // 4

    // Branch at the explicit, non-leaf entry a0 → keep only [q0, a0].
    let new_path = m
        .create_branched_session(&a0, &lay)
        .unwrap()
        .expect("a persisted branch returns the new file path");

    assert_ne!(new_path, old_file, "branched session lives in a new file");
    assert_eq!(m.session_file(), Some(new_path.as_path()), "manager mutated in place to the branch");
    assert_eq!(m.entries().len(), 2, "only the root→a0 path is retained");
    assert_eq!(m.leaf_id().cloned(), m.entries().last().map(Entry::id));
    assert_eq!(m.header().parent_session.as_deref(), Some(old_file.to_string_lossy().as_ref()));
    // a0 is an assistant → the branch is written eagerly.
    assert!(new_path.exists(), "assistant-bearing branch is written eagerly");

    // The previous file is untouched on disk (Pi never rewrites it): all 4 entries remain.
    let reopened = SessionManager::open(&old_file).unwrap();
    assert_eq!(reopened.entries().len(), old_count);
}

// ---- gap-analysis 05: SessionLayout literal mode + open-nonexistent (Findings 1/2/3) -------------

#[test]
fn f3_session_layout_literal_is_used_verbatim() {
    // Finding 3. Pi uses an explicit `--session-dir` LITERALLY (`sessionDir ? normalizePath(sessionDir)
    // : getDefaultSessionDir(cwd)`, session-manager.ts:1430,1447,1457). `literal` must NOT re-encode.
    let cwd = PathBuf::from("/work/proj");
    let root = PathBuf::from("/tmp/custom-sessions");
    assert_eq!(
        SessionLayout::literal(root.clone(), cwd.clone()).dir(),
        root,
        "an explicit --session-dir must be used verbatim, with no --<cwd>-- suffix",
    );
    // The default constructor still encodes (the default root case is unchanged).
    assert_eq!(
        SessionLayout::new(root.clone(), cwd.clone()).dir(),
        root.join(cyrup_session::encode_cwd(&cwd)),
        "the default (non-explicit) root is still cwd-encoded",
    );
}

#[test]
fn f1_branch_reuses_the_open_dir_without_re_encoding() {
    // Finding 1. Pi's `createBranchedSession` reuses `this.getSessionDir()` — the dir fixed once at
    // construction, never re-encoded on branch (session-manager.ts:918-920,1343). A branch caller that
    // takes the currently-open session file's OWN directory (already `<root>/--<cwd>--`) must feed it
    // through the LITERAL layout, or the branch nests one level too deep and is orphaned from listing.
    let cwd = PathBuf::from("/work/proj");
    let open_dir = PathBuf::from("/tmp/root").join(cyrup_session::encode_cwd(&cwd));
    assert_eq!(
        SessionLayout::literal(open_dir.clone(), cwd.clone()).dir(),
        open_dir,
        "a fork must land in the SAME directory as the session it branched from",
    );
}

#[test]
fn f1_forked_file_stays_in_the_listing_dir_end_to_end() {
    // Finding 1, end to end over the real manager: a persisted branch lands in the SAME directory the
    // listing scans, so `newest_session`/`list` still see it (they re-derive a fresh single-level
    // layout from the sessions root). Mirrors AgentSession::fork/clone_at's `branch_layout` reuse.
    let dir = tempfile::tempdir().unwrap();
    let cwd = PathBuf::from("/proj/f1");
    let lay = SessionLayout::new(dir.path().to_path_buf(), cwd.clone());
    let mut m = SessionManager::create(&cwd, &lay, NewSessionOpts::default()).unwrap();
    m.append_message(user("q0")).unwrap();
    m.append_message(asst()).unwrap();
    let original = m.session_file().unwrap().to_path_buf();
    let list_dir = original.parent().unwrap().to_path_buf(); // <root>/--<cwd>--

    // Branch the way the service callers do: reuse the open file's OWN parent dir, LITERALLY.
    let branch_layout = SessionLayout::literal(list_dir.clone(), cwd.clone());
    let leaf = m.leaf_id().cloned().unwrap();
    let branched = m.create_branched_session(&leaf, &branch_layout).unwrap().unwrap();

    assert_eq!(
        branched.parent().unwrap(),
        list_dir,
        "the branch must live in the same dir the listing scans, not one level deeper",
    );
    let listed = cyrup_session::listing::list(&lay);
    let paths: Vec<PathBuf> = listed.iter().map(|s| s.path.clone()).collect();
    assert!(paths.contains(&original), "original session is listed");
    assert!(paths.contains(&branched), "Finding 1: the branched session must be visible to listing");
}

#[test]
fn f2_open_nonexistent_path_creates_a_fresh_session() {
    // Finding 2. Pi treats a not-yet-existing `--session <path>` as a NEW session anchored there
    // (`loadEntriesFromFile` returns [] for a missing file → `setSessionFile`'s !existsSync branch →
    // newSession + preserve the explicit path, session-manager.ts:489-491,843-847). It must NOT error.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("brand-new-notes.jsonl");
    assert!(!path.exists());
    let m = SessionManager::open(&path).expect("opening a nonexistent --session path creates a fresh session");
    assert_eq!(m.session_file(), Some(path.as_path()), "the explicit path is preserved verbatim");
    assert!(m.entries().is_empty(), "a fresh session starts empty");
    // Fresh + no assistant yet ⇒ deferred flush, exactly like `newSession` (no file written yet).
    assert!(!path.exists(), "the file is deferred until the first assistant message (Pi parity)");
}

// ---------------------------------------------------------------- SESS-001 ---------------------

#[test]
fn sess001_null_or_missing_content_is_normalized_to_empty_not_dropped() {
    // Pi `sessionEntryToContextMessages` (`session-manager.ts:382-395`): "Session files are parsed
    // without validation; old versions, forks, or hand-edited files can contain messages with
    // null/missing content", then
    //   if ((role === "user" || role === "assistant" || role === "toolResult") && content == null)
    //       return [{ ...message, content: [] }];
    // `== null` also matches `undefined`, so an ABSENT `content` key normalizes the same way. cyrup
    // must keep the turn (as a typed entry with empty content), not demote it to `Entry::Unknown`
    // and silently drop it from LLM context, compaction input and token accounting.
    let mut asst_null = serde_json::to_value(assistant_text("ignored")).unwrap();
    asst_null["content"] = Value::Null;
    let mut asst_missing = serde_json::to_value(assistant_text("ignored")).unwrap();
    asst_missing.as_object_mut().unwrap().remove("content");

    let cases: Vec<(&str, serde_json::Value)> = vec![
        ("user/null", json!({ "role": "user", "content": null, "timestamp": 0 })),
        ("user/missing", json!({ "role": "user", "timestamp": 0 })),
        ("assistant/null", asst_null),
        ("assistant/missing", asst_missing),
        (
            "toolResult/null",
            json!({
                "role": "toolResult",
                "toolCallId": "tc-1",
                "toolName": "read",
                "content": null,
                "timestamp": 0,
            }),
        ),
        (
            "toolResult/missing",
            json!({
                "role": "toolResult",
                "toolCallId": "tc-1",
                "toolName": "read",
                "timestamp": 0,
            }),
        ),
    ];

    for (label, message) in cases {
        let entry: Entry = serde_json::from_value(json!({
            "type": "message",
            "id": "aaaaaaaa",
            "parentId": null,
            "timestamp": "2026-01-01T00:00:00Z",
            "message": message,
        }))
        .unwrap();
        assert!(
            matches!(entry, Entry::Known(KnownEntry::Message { .. })),
            "{label}: must parse as a typed message entry, got {entry:?}"
        );

        // Observable effect: the turn reaches the LLM context with an EMPTY content array.
        let path = [&entry];
        let msgs = build_context_messages(&path);
        assert_eq!(msgs.len(), 1, "{label}: the turn must not vanish from context");
        let content = match &msgs[0] {
            Message::User { content, .. } | Message::ToolResult { content, .. } => content.clone(),
            Message::Assistant(a) => a.content.clone(),
        };
        assert!(content.is_empty(), "{label}: content normalizes to [], got {content:?}");
    }
}

#[test]
fn sess001_normalized_empty_content_still_counts_as_a_cut_point_and_round_trips() {
    // The normalization must produce a REAL entry, not a special case: it participates in the
    // cut-point walk (Pi's `findValidCutPoints` sees a `message` entry whose role is not
    // toolResult) and re-serializes as the array form Pi always writes back.
    let entry: Entry = serde_json::from_value(json!({
        "type": "message",
        "id": "bbbbbbbb",
        "parentId": null,
        "timestamp": "2026-01-01T00:00:00Z",
        "message": { "role": "user", "content": null, "timestamp": 0 },
    }))
    .unwrap();
    assert_eq!(find_valid_cut_points(std::slice::from_ref(&entry), 0, 1), vec![0]);
    let line = entry.to_line().unwrap();
    assert!(line.contains("\"content\":[]"), "writes the array form back, got {line}");
}

// ------------------------------------- SESS-015 unresolvable firstKeptEntryIndex ---------------

/// Write a realistic v1 (no `version`, no `id`/`parentId`) session file whose compaction points at
/// its first-kept entry by NUMERIC `firstKeptEntryIndex`, and open it.
fn write_v1_session_with_first_kept_index(dir: &std::path::Path, index: Value) -> PathBuf {
    let path = dir.join("v1.jsonl");
    let msg = |ts: &str, body: &str| {
        json!({
            "type": "message",
            "timestamp": ts,
            "message": serde_json::to_value(AgentMessage::Core(user(body))).unwrap(),
        })
        .to_string()
    };
    let mut lines = vec![
        // v1 header: no `version` key at all.
        json!({
            "type": "session",
            "id": "sess-v1-sess015",
            "timestamp": "2026-01-01T00:00:00Z",
            "cwd": "/proj/sess015",
        })
        .to_string(),
        // file index 1..=3 — the pre-compaction history that the compaction summarized away.
        msg("2026-01-01T00:00:01Z", "OLD-ONE how do I parse a PEM key"),
        msg("2026-01-01T00:00:02Z", "OLD-TWO now add error handling"),
        msg("2026-01-01T00:00:03Z", "OLD-THREE and write the tests"),
        // file index 4 — the compaction itself.
        json!({
            "type": "compaction",
            "timestamp": "2026-01-01T00:00:04Z",
            "summary": "THE SUMMARY of everything before the cut",
            "tokensBefore": 4242,
            "firstKeptEntryIndex": index,
        })
        .to_string(),
        // file index 5 — after the compaction, always kept.
        msg("2026-01-01T00:00:05Z", "NEW-ONE what did we decide"),
    ];
    lines.push(String::new());
    std::fs::write(&path, lines.join("\n")).unwrap();
    path
}

/// SESS-015 — a v1 compaction whose `firstKeptEntryIndex` CANNOT be resolved must still be read as
/// a compaction: the summary reaches the model and the summarized history stays OUT of the context.
///
/// Pi `migrateV1ToV2` (`session-manager.ts:245-255`) deletes `firstKeptEntryIndex` unconditionally
/// but assigns `firstKeptEntryId` only when the index resolves to a non-`session` entry, so index 0
/// (the header) and an out-of-range index leave the key ABSENT. Pi keeps the entry as a compaction
/// regardless, and `buildContextEntries`' `entry.id === compaction.firstKeptEntryId` test
/// (`session-manager.ts:445`) never matches an absent id, so its `0..compactionIdx` loop pushes
/// NOTHING. The harness fork states the same contract as an explicit guard —
/// `if (compaction.firstKeptEntryId)` (`agent/src/harness/session/session.ts:80`).
///
/// Before the fix, `first_kept_entry_id: EntryId` (non-optional) made the entry fail to parse as a
/// `KnownEntry`, so it landed as `Entry::Unknown`: `latest_compaction` returned `None`, both
/// builders took the "no compaction" arm, and the WHOLE pre-compaction history was re-admitted —
/// while the summary itself, being `Unknown`, contributed nothing at all.
#[test]
fn sess015_unresolvable_first_kept_index_keeps_the_compacted_history_out_of_context() {
    for (label, index) in [("header (index 0)", json!(0)), ("out of range", json!(99))] {
        let dir = tempfile::tempdir().unwrap();
        let path = write_v1_session_with_first_kept_index(dir.path(), index);
        let m = SessionManager::open(&path).unwrap();

        // Observable behavior #1 — the LLM-rendered context.
        let texts: Vec<String> = m.build_context().messages.iter().map(text_of).collect();
        let joined = texts.join("\n---\n");
        assert!(
            joined.contains("THE SUMMARY of everything before the cut"),
            "[{label}] the summary must reach the model; got:\n{joined}"
        );
        for dropped in ["OLD-ONE", "OLD-TWO", "OLD-THREE"] {
            assert!(
                !joined.contains(dropped),
                "[{label}] {dropped} was summarized away and must NOT be re-admitted; got:\n{joined}"
            );
        }
        assert!(
            joined.contains("NEW-ONE what did we decide"),
            "[{label}] entries after the compaction are always kept; got:\n{joined}"
        );
        assert_eq!(
            texts.len(),
            2,
            "[{label}] exactly the summary + the post-compaction message: {texts:?}"
        );

        // Observable behavior #2 — the RAW projection compaction/token accounting runs on.
        let raw = m.build_context_raw();
        assert_eq!(raw.len(), 2, "[{label}] raw context is summary + post-compaction: {raw:?}");
        assert!(
            matches!(raw.first(), Some(AgentMessage::CompactionSummary(_))),
            "[{label}] the raw context leads with the compaction summary: {raw:?}"
        );

        // Mechanism: the entry survives as an INTERPRETED compaction (it used to degrade to
        // `Entry::Unknown`, which is what re-admitted the history), carrying NO `firstKeptEntryId`.
        let path_entries = m.branch_path(None);
        let comp = path_entries
            .iter()
            .find(|e| matches!(e, Entry::Known(KnownEntry::Compaction { .. })))
            .unwrap_or_else(|| panic!("[{label}] compaction must parse as a KnownEntry"));
        let line = comp.to_line().unwrap();
        assert!(
            !line.contains("firstKeptEntryId"),
            "[{label}] the index is unresolvable, so no id is assigned: {line}"
        );
        assert!(
            !line.contains("firstKeptEntryIndex"),
            "[{label}] the v1 index field is dropped on migration: {line}"
        );
    }
}

/// The resolvable case must be untouched by the SESS-015 fix: index 2 → the second history entry,
/// so `OLD-TWO` onward are kept and only `OLD-ONE` is dropped.
#[test]
fn sess015_resolvable_first_kept_index_still_keeps_the_tail_of_the_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_v1_session_with_first_kept_index(dir.path(), json!(2));
    let m = SessionManager::open(&path).unwrap();
    let joined =
        m.build_context().messages.iter().map(text_of).collect::<Vec<_>>().join("\n---\n");
    assert!(joined.contains("THE SUMMARY of everything before the cut"), "{joined}");
    assert!(!joined.contains("OLD-ONE"), "index 2 cuts before OLD-TWO: {joined}");
    assert!(joined.contains("OLD-TWO"), "{joined}");
    assert!(joined.contains("OLD-THREE"), "{joined}");
    assert!(joined.contains("NEW-ONE"), "{joined}");
}
