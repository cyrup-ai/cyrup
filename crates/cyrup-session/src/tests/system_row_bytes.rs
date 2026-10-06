//! CODE-014 — a system row pi wrote survives a cyrup REWRITE byte for byte.
//!
//! pi writes a system message in one of two key orders, depending on who built it, and a cyrup
//! rewrite (the branch export, a forked file) must give each back unchanged:
//!
//! * a `message` entry's row comes from the agent loop's `withToolChanges`
//!   (`packages/agent/src/agent-loop.ts:368-375` @v1.0.0): the pending message's own keys, ending in
//!   `timestamp`, then `toolsAdded`, then `toolsRemoved`;
//! * a `compaction` entry's `systemMessage` is a replayed snapshot, `getCurrentSystemMessage`
//!   (`packages/ai/src/utils/transcript.ts:95-100`) with `timestamp` overwritten in place
//!   (`session-manager.ts:1270-1283`): `toolsAdded` BEFORE `timestamp`.
//!
//! The assertions are on BYTES. A comparison of parsed JSON passes whatever the order is, and the
//! order is the only thing a rewrite can change.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::SessionManager;

const HEADER: &str = r#"{"type":"session","version":3,"id":"0197a0a0-0000-7000-8000-000000000042","timestamp":"2026-08-08T00:00:00.000Z","cwd":"/pi/work"}"#;

/// A system row as pi's loop writes it for a session's first prompt: the prompt update
/// (`role, content, sections, timestamp`), then the tool keys appended after it.
const SYSTEM_ROW: &str = concat!(
    r#"{"type":"message","id":"s1","parentId":null,"timestamp":"2026-08-08T00:00:01.000Z","message":{"#,
    r#""role":"system","content":"","#,
    r#""sections":{"preamble":"You are an expert coding assistant operating inside pi, a coding agent harness.","#,
    r#""tools":"<tools>\n- read: Read file contents\n\nIn addition to the tools above, you may have access to other custom tools depending on the project.\n</tools>","#,
    r#""cwd":"<cwd>\n/pi/work\n</cwd>","pi_only":"<pi_only>\nonly pi writes this\n</pi_only>"},"#,
    r#""timestamp":1754611201000,"#,
    r#""toolsAdded":[{"name":"read","description":"Read file contents","parameters":{"type":"object","properties":{"path":{"type":"string"}}}}],"#,
    r#""toolsRemoved":[{"name":"old"}]}}"#,
);

/// A later row that only declares a tool: no sections at all.
const TOOLS_ONLY_ROW: &str = concat!(
    r#"{"type":"message","id":"s2","parentId":"u1","timestamp":"2026-08-08T00:00:03.000Z","message":{"#,
    r#""role":"system","content":"","timestamp":1754611203000,"#,
    r#""toolsAdded":[{"name":"write","description":"Write a file","parameters":{"type":"object"}}]}}"#,
);

const USER_ROW: &str = r#"{"type":"message","id":"u1","parentId":"s1","timestamp":"2026-08-08T00:00:02.000Z","message":{"role":"user","content":[{"type":"text","text":"hello"}],"timestamp":1754611202000}}"#;

/// A compaction entry as pi writes it, carrying the replayed snapshot: `toolsAdded` BEFORE
/// `timestamp`.
const COMPACTION_ROW: &str = concat!(
    r#"{"type":"compaction","id":"c1","parentId":"s2","timestamp":"2026-08-08T00:00:04.000Z","#,
    r#""summary":"S","firstKeptEntryId":"u1","tokensBefore":123,"#,
    r#""systemMessage":{"role":"system","content":"","#,
    r#""sections":{"preamble":"You are an expert coding assistant operating inside pi, a coding agent harness."},"#,
    r#""toolsAdded":[{"name":"read","description":"Read file contents","parameters":{"type":"object"}},{"name":"write","description":"Write a file","parameters":{"type":"object"}}],"#,
    r#""timestamp":1754611204000}}"#,
);

fn pi_file() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("pi.jsonl");
    let lines = [HEADER, SYSTEM_ROW, USER_ROW, TOOLS_ONLY_ROW, COMPACTION_ROW];
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    (tmp, path)
}

/// The branch export re-serializes every entry on the branch from the typed model. Every system row
/// and the compaction snapshot that pi wrote must come back as the very bytes pi wrote.
#[test]
fn a_rewrite_gives_every_pi_system_row_back_byte_for_byte() {
    let (_tmp, path) = pi_file();
    let manager = SessionManager::open(&path).unwrap();

    let mut exported = Vec::new();
    manager.export_branch_jsonl(&mut exported).unwrap();
    let exported = String::from_utf8(exported).unwrap();
    let rewritten: Vec<&str> = exported.lines().skip(1).collect(); // after the fresh header

    assert_eq!(
        rewritten,
        [SYSTEM_ROW, USER_ROW, TOOLS_ONLY_ROW, COMPACTION_ROW],
        "a cyrup rewrite must not reorder a pi row's keys, nor drop its unknown `pi_only` section"
    );
}

/// The whole-tree dump is the other rewrite path; it must agree.
#[test]
fn the_whole_tree_dump_agrees() {
    let (_tmp, path) = pi_file();
    let manager = SessionManager::open(&path).unwrap();

    let mut dump = Vec::new();
    manager.export_jsonl(&mut dump).unwrap();
    let dump = String::from_utf8(dump).unwrap();
    let rows: Vec<&str> = dump.lines().skip(1).collect();
    assert_eq!(rows, [SYSTEM_ROW, USER_ROW, TOOLS_ONLY_ROW, COMPACTION_ROW]);
}
