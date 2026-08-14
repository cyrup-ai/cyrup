//! AGENT-004/005 — the widened `Message::ToolResult` on the ON-DISK session JSONL.
//!
//! `usage` / `addedToolNames` are appended to the append-only session file, so both directions of
//! the compatibility contract are pinned here:
//!   * NEW code writes them; a re-import recovers them intact (no loss across export→import).
//!   * NEW code reading an OLD file (keys absent) re-exports the file byte-identically.
//!   * OLD code reading a NEW file parses the entry (no `deny_unknown_fields` anywhere on the
//!     path), so the line never demotes to `Entry::Unknown` — it just drops the two keys.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::io::Write;

use cyrup_core::{Content, Message, Usage};
use crate::{NewSessionOpts, SessionManager};

fn tool_result(usage: Option<Usage>, added: &[&str]) -> Message {
    Message::ToolResult {
        tool_call_id: "tc1".into(),
        tool_name: "loader".into(),
        content: vec![Content::text("ok")],
        is_error: false,
        details: None,
        usage,
        added_tool_names: added.iter().map(|s| (*s).to_string()).collect(),
        timestamp: 7,
    }
}

fn usage() -> Usage {
    Usage { input: 11, output: 22, total_tokens: 33, ..Usage::default() }
}

fn export(m: &SessionManager) -> String {
    let mut buf: Vec<u8> = Vec::new();
    m.export_jsonl(&mut buf).unwrap();
    String::from_utf8(buf).unwrap()
}

/// Every JSONL line as a JSON value (key-order-insensitive comparison).
fn values(jsonl: &str) -> Vec<serde_json::Value> {
    jsonl
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn import(jsonl: &str) -> SessionManager {
    let mut f =
        tempfile::Builder::new().prefix("cyrup-trf-").suffix(".jsonl").tempfile().unwrap();
    f.write_all(jsonl.as_bytes()).unwrap();
    f.flush().unwrap();
    SessionManager::import_jsonl(f.path()).unwrap()
}

/// The two fields survive append → export → import → export with byte-for-byte equality.
#[test]
fn usage_and_added_tool_names_survive_the_session_file_round_trip() {
    let cwd = std::env::temp_dir();
    let mut m = SessionManager::in_memory(&cwd, NewSessionOpts::default()).unwrap();
    m.append_message(tool_result(Some(usage()), &["late"])).unwrap();

    let first = export(&m);
    assert!(first.contains(r#""addedToolNames":["late"]"#), "{first}");
    assert!(first.contains(r#""usage":{"#), "{first}");

    let reimported = import(&first);
    let second = export(&reimported);
    assert_eq!(second, first, "export → import → export is byte-identical");

    // And the recovered value is the real thing, not a default.
    let recovered: Vec<Message> = reimported
        .entries()
        .iter()
        .filter_map(|e| match e {
            crate::Entry::Known(crate::KnownEntry::Message {
                message: crate::agent_message::AgentMessage::Core(c),
                ..
            }) => Some(c.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(recovered.len(), 1);
    match &recovered[0] {
        Message::ToolResult { usage: u, added_tool_names, .. } => {
            assert_eq!(u.as_ref(), Some(&usage()), "usage recovered from disk");
            assert_eq!(added_tool_names, &vec!["late".to_string()], "anchor recovered from disk");
        }
        other => panic!("expected a tool result, got {other:?}"),
    }
}

/// A tool result WITHOUT the fields writes neither key, so a session produced by the new code is
/// byte-identical to one produced before the change whenever no tool reports either.
#[test]
fn absent_fields_write_no_keys() {
    let cwd = std::env::temp_dir();
    let mut m = SessionManager::in_memory(&cwd, NewSessionOpts::default()).unwrap();
    m.append_message(tool_result(None, &[])).unwrap();
    let jsonl = export(&m);
    let line = jsonl.lines().find(|l| l.contains("toolResult")).unwrap();
    assert!(!line.contains("addedToolNames"), "{line}");
    assert!(!line.contains(r#""usage""#), "{line}");
}

/// BACKWARD — a pre-change session file loads and re-exports unchanged under the new code.
#[test]
fn pre_change_session_file_re_exports_unchanged() {
    let cwd = std::env::temp_dir();
    let mut m = SessionManager::in_memory(&cwd, NewSessionOpts::default()).unwrap();
    m.append_message(tool_result(None, &[])).unwrap();
    // This export has no `usage`/`addedToolNames` keys anywhere, i.e. it IS the old on-disk shape.
    let old_file = export(&m);
    assert!(!old_file.contains("addedToolNames"));

    let reimported = import(&old_file);
    assert_eq!(export(&reimported), old_file, "old file re-exports byte-identically");
}

/// FORWARD — old code reading a new file. The pre-change reader is modelled by parsing each entry
/// as a bare `serde_json::Value` and stripping the two keys: nothing on the read path declares
/// `deny_unknown_fields`, so the stripped line still parses as a known entry and the session does
/// NOT demote it to `Entry::Unknown`.
#[test]
fn new_file_read_by_pre_change_shape_stays_a_known_entry() {
    let cwd = std::env::temp_dir();
    let mut m = SessionManager::in_memory(&cwd, NewSessionOpts::default()).unwrap();
    m.append_message(tool_result(Some(usage()), &["late"])).unwrap();
    let new_file = export(&m);

    // What a pre-change writer would have produced from the same session.
    let downgraded: String = new_file
        .lines()
        .map(|l| {
            let mut v: serde_json::Value = serde_json::from_str(l).unwrap();
            if let Some(msg) = v.get_mut("message").and_then(|x| x.as_object_mut()) {
                msg.remove("usage");
                msg.remove("addedToolNames");
            }
            serde_json::to_string(&v).unwrap()
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";

    let reimported = import(&downgraded);
    // Every entry is still a KNOWN entry (an unparseable body would land in `Entry::Unknown`).
    let unknown = reimported
        .entries()
        .iter()
        .filter(|e| matches!(e, crate::Entry::Unknown(_)))
        .count();
    assert_eq!(unknown, 0, "the downgraded file has no unknown entries");
    // Compare as JSON values, not bytes: the downgrade step above rebuilt each line through
    // `serde_json::Value` (a BTreeMap), which alphabetizes keys. The claim under test is that no
    // DATA is lost, not that a hand-rewritten line keeps cyrup's key order.
    assert_eq!(values(&export(&reimported)), values(&downgraded), "and re-exports losslessly");

    // The NEW reader recovers defaults, not garbage.
    let tr = reimported
        .entries()
        .iter()
        .find_map(|e| match e {
            crate::Entry::Known(crate::KnownEntry::Message {
                message: crate::agent_message::AgentMessage::Core(
                    c @ Message::ToolResult { .. },
                ),
                ..
            }) => Some(c.clone()),
            _ => None,
        })
        .expect("the tool result survived");
    match tr {
        Message::ToolResult { usage: u, added_tool_names, .. } => {
            assert_eq!(u, None);
            assert!(added_tool_names.is_empty());
        }
        other => panic!("expected a tool result, got {other:?}"),
    }
}

// ============================================================== the `Entry::Unknown` lever ====

/// A byte-exact replica of the PRE-CHANGE `Message::ToolResult` arm — same serde attributes, minus
/// the two new fields, and (as in the real type) with no `deny_unknown_fields`.
#[derive(serde::Deserialize)]
#[serde(tag = "role", rename_all = "camelCase", rename_all_fields = "camelCase")]
enum OldMessage {
    ToolResult {
        tool_name: String,
        #[serde(default)]
        is_error: bool,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        details: Option<serde_json::Value>,
        timestamp: i64,
    },
}

/// The pre-change `KnownEntry::Message` arm, so the forward-compat claim is tested at the ENTRY
/// level (where the `Entry::Unknown` demotion decision is actually made), not just the message level.
#[derive(serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
enum OldKnownEntry {
    Message { id: String, message: OldMessage },
}

/// FORWARD, at the entry level. `Entry`'s `Deserialize` demotes a line to `Entry::Unknown` only when
/// a KNOWN `type` tag has a body that FAILS to parse (entry.rs `from_value::<KnownEntry>(v)` → `Err`).
/// So the question that decides whether an old build silently loses a new session file is precisely:
/// does the pre-change `KnownEntry` still parse a line the new code wrote? It does — the two extra
/// keys are ignored, the entry stays `Known`, and only the two values are dropped.
#[test]
fn a_new_code_entry_still_parses_under_the_pre_change_entry_schema() {
    let cwd = std::env::temp_dir();
    let mut m = SessionManager::in_memory(&cwd, NewSessionOpts::default()).unwrap();
    m.append_message(tool_result(Some(usage()), &["late"])).unwrap();
    let jsonl = export(&m);

    let line = jsonl.lines().find(|l| l.contains("addedToolNames")).unwrap();
    // Sanity: the line really does carry both new keys.
    assert!(line.contains(r#""usage""#), "{line}");

    let old: OldKnownEntry = serde_json::from_str(line)
        .unwrap_or_else(|e| panic!("the pre-change entry schema rejected a new-code line: {e}\n{line}"));
    let OldKnownEntry::Message {
        id,
        message: OldMessage::ToolResult { tool_name, is_error, details, timestamp },
    } = old;
    // Every field the old reader DID model still arrives intact — the widening cost it exactly the
    // two keys it never knew about, and nothing else.
    assert!(!id.is_empty(), "the entry id survived");
    assert_eq!(tool_name, "loader", "the old reader still sees the real payload");
    assert!(!is_error);
    assert_eq!(details, None);
    assert_eq!(timestamp, 7);
}

/// THE `Entry::Unknown` LEVER ITSELF. An entry whose `type` this build does not recognize is kept
/// VERBATIM and re-emitted unchanged (entry.rs: `Entry::Unknown(v)` + a passthrough `Serialize`).
/// Pinned here next to the widened tool result because it is the mechanism that makes the on-disk
/// format extensible in the first place: the same file can carry a tool result the reader fully
/// understands and an entry it has never heard of, and a load+save loses neither.
#[test]
fn an_unknown_entry_and_a_widened_tool_result_share_a_file_losslessly() {
    let cwd = std::env::temp_dir();
    let mut m = SessionManager::in_memory(&cwd, NewSessionOpts::default()).unwrap();
    m.append_message(tool_result(Some(usage()), &["late"])).unwrap();
    let base = export(&m);

    // Splice in an entry with a `type` no cyrup build knows, carrying a nested object so a
    // re-serialization that reordered or re-shaped anything would show up.
    let unknown = r#"{"type":"someFutureThing","id":"zzz","parentId":null,"payload":{"b":2,"a":[1,{"deep":true}]}}"#;
    let spliced = format!("{}{unknown}\n", base);

    let reimported = import(&spliced);
    let out = export(&reimported);

    let kept = out
        .lines()
        .find(|l| l.contains("someFutureThing"))
        .unwrap_or_else(|| panic!("the unknown entry was dropped:\n{out}"));
    // Verbatim in VALUE, not in bytes: `Entry::Unknown` parks the line in a `serde_json::Value`,
    // whose object is a `BTreeMap` (serde_json is built without `preserve_order`), so a passthrough
    // re-serialization alphabetizes keys. No field is added, dropped, coerced or re-nested — which
    // is the guarantee the format actually needs. Worth knowing before treating a session file as
    // byte-stable across a load+save: only entries this build fully models keep their key order.
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(kept).unwrap(),
        serde_json::from_str::<serde_json::Value>(unknown).unwrap(),
        "the unknown entry round-trips verbatim"
    );
    // And the entry the reader DOES understand kept both new keys through the same round trip.
    let anchored = out.lines().find(|l| l.contains("addedToolNames")).unwrap();
    assert!(anchored.contains(r#""addedToolNames":["late"]"#), "{anchored}");
    assert!(anchored.contains(r#""usage""#), "{anchored}");
    assert_eq!(values(&out), values(&spliced), "nothing else moved either");
}
