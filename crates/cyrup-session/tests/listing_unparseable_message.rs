//! `/resume`-listing parity for `message` entries whose BODY does not fit cyrup's typed
//! [`cyrup_session::agent_message::AgentMessage`].
//!
//! Pi's listing scan is untyped: `parseSessionEntryLine` is a bare `JSON.parse`
//! (`session-manager.ts:503-511`), so `if (entry.type !== "message") continue; messageCount++`
//! (`:717-718`) counts on the TAG ALONE, and `extractTextContent` (`:662-671`) then reads
//! `message.role` / `message.content` straight off the parsed JSON. cyrup's `Entry` deserializer
//! demotes a known tag with an unparseable body to `Entry::Unknown` (`entry.rs:277-279`), so the
//! listing must handle that shape explicitly or such entries vanish from `message_count`, from the
//! `/resume` search text and from the preview line.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use cyrup_session::listing;
use tempfile::TempDir;

/// A session file whose three `message` entries are:
///   1. a plain typed user message (parses),
///   2. a user message carrying a content block type cyrup does not know — Pi keeps its `text`
///      blocks, cyrup's per-role `de_user_content` rejects the array (`message.rs:552`),
///   3. a legacy pre-v3 `hookMessage`-role message (`migrate.rs:29-32`), which never parses.
fn write_session(dir: &std::path::Path, cwd: &str) -> std::path::PathBuf {
    let path = dir.join("2026-01-01T00-00-00-000Z_0193f0e1-0000-7000-8000-000000000001.jsonl");
    let lines = [
        format!(
            r#"{{"type":"session","version":3,"id":"0193f0e1-0000-7000-8000-000000000001","timestamp":"2026-01-01T00:00:00.000Z","cwd":"{cwd}"}}"#
        ),
        r#"{"type":"message","id":"aaaaaaa1","parentId":null,"timestamp":"2026-01-01T00:00:01.000Z","message":{"role":"user","content":[{"type":"text","text":"typed hello"}],"timestamp":1767225601000}}"#
            .to_string(),
        r#"{"type":"message","id":"aaaaaaa2","parentId":"aaaaaaa1","timestamp":"2026-01-01T00:00:02.000Z","message":{"role":"user","content":[{"type":"text","text":"untyped hello"},{"type":"videoRef","url":"file:///clip.mp4"}],"timestamp":1767225602000}}"#
            .to_string(),
        r#"{"type":"message","id":"aaaaaaa3","parentId":"aaaaaaa2","timestamp":"2026-01-01T00:00:03.000Z","message":{"role":"hookMessage","content":[{"type":"text","text":"legacy hook"}],"timestamp":1767225603000}}"#
            .to_string(),
    ];
    std::fs::write(&path, format!("{}\n", lines.join("\n"))).unwrap();
    path
}

/// Pi `messageCount++` fires on the tag alone (`session-manager.ts:717-718`), so all three
/// `message` entries count even though two of them have bodies cyrup cannot type.
#[test]
fn message_count_includes_entries_whose_body_fails_to_type() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    let dir = tmp.path().join("sessions");
    std::fs::create_dir_all(&dir).unwrap();
    let path = write_session(&dir, &cwd.display().to_string());

    let listed = listing::list_in_dir(&dir, None, None);
    let info = listed.iter().find(|s| s.path == path).expect("session listed");
    assert_eq!(
        info.message_count, 3,
        "Pi counts every `message`-tagged entry; got {} for {:?}",
        info.message_count, info.path
    );
}

/// Pi's `extractTextContent` keeps the `text` blocks of a user/assistant message regardless of the
/// other blocks present (`session-manager.ts:662-671`), so the untyped entry's text must reach the
/// `/resume` search haystack. The `hookMessage` role is neither `user` nor `assistant`
/// (`:726-727`), so it contributes NO text — exactly as after a v2→v3 migration renames it to the
/// equally non-core `custom` role.
#[test]
fn search_text_includes_untyped_user_message() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    let dir = tmp.path().join("sessions");
    std::fs::create_dir_all(&dir).unwrap();
    let path = write_session(&dir, &cwd.display().to_string());

    let listed = listing::list_in_dir(&dir, None, None);
    let info = listed.iter().find(|s| s.path == path).expect("session listed");
    assert!(
        info.all_messages_text.contains("typed hello"),
        "typed message text missing: {:?}",
        info.all_messages_text
    );
    assert!(
        info.all_messages_text.contains("untyped hello"),
        "untyped user message text missing from the search haystack: {:?}",
        info.all_messages_text
    );
    assert!(
        !info.all_messages_text.contains("legacy hook"),
        "a non-core role must not contribute text: {:?}",
        info.all_messages_text
    );
}

/// The preview line is the FIRST core user message's text (Pi `if (!firstMessage && role ===
/// "user")`, `session-manager.ts:734-736`). An untyped entry that is the only user message must
/// still supply it rather than leaving the row as `(no messages)`.
#[test]
fn first_message_falls_back_to_an_untyped_user_entry() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    let dir = tmp.path().join("sessions");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("2026-01-01T00-00-00-000Z_0193f0e1-0000-7000-8000-000000000002.jsonl");
    let cwd_s = cwd.display().to_string();
    let lines = [
        format!(
            r#"{{"type":"session","version":3,"id":"0193f0e1-0000-7000-8000-000000000002","timestamp":"2026-01-01T00:00:00.000Z","cwd":"{cwd_s}"}}"#
        ),
        r#"{"type":"message","id":"bbbbbbb1","parentId":null,"timestamp":"2026-01-01T00:00:01.000Z","message":{"role":"user","content":[{"type":"text","text":"only untyped"},{"type":"videoRef","url":"file:///clip.mp4"}],"timestamp":1767225601000}}"#
            .to_string(),
    ];
    std::fs::write(&path, format!("{}\n", lines.join("\n"))).unwrap();

    let listed = listing::list_in_dir(&dir, None, None);
    let info = listed.iter().find(|s| s.path == path).expect("session listed");
    assert_eq!(info.first_message, "only untyped", "preview line must use the untyped user entry");
}
