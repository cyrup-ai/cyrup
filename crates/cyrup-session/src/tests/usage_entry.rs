//! SESS-051 residual — the `usage` session entry (pi `UsageEntry`, `session-manager.ts:80-89`
//! @v1.0.4), the entry type prompt-cache warming persists its spend as.
//!
//! Four things have to hold and only one of them is enforced by the compiler:
//!
//! 1. a pi-written `usage` line re-serializes BYTE for byte (key order included);
//! 2. it PROMOTES to [`KnownEntry::Usage`] rather than demoting to `Entry::Unknown`;
//! 3. [`SessionManager::append_usage`] advances the branch leaf, so the next entry parents to it
//!    (pi's `_appendEntry`, `:1191-1196`);
//! 4. it is context-INVISIBLE — projects no message, costs 0 tokens, and is folded past by the
//!    compaction cut-point back-scan.
//!
//! (4) arrives entirely through catch-alls (`context::push_as_raw`'s `_ => {}`,
//! `context_message_role`'s `_ => None`, `compaction::cutpoint`'s `is_context_visible`), so no
//! compiler error will ever force a reviewer to look at it. That is what the last test is for.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::agent_message::MessageRole;
use cyrup_core::{Content, Message, Usage};

use crate::SessionManager;
use crate::entry::{Entry, KnownEntry};

const HEADER: &str = r#"{"type":"session","version":3,"id":"0197a0a0-0000-7000-8000-000000000042","timestamp":"2026-10-08T00:00:00.000Z","cwd":"/pi/work"}"#;

/// A `usage` row exactly as pi's `appendUsage` writes it (`session-manager.ts:1244-1259` @v1.0.4):
/// `type, id, parentId, timestamp` from `SessionEntryBase` (`:57-62`), then the payload in
/// declaration order `kind, provider, model, usage`, then `note` spread in last.
const USAGE_ROW_WITH_NOTE: &str = concat!(
    r#"{"type":"usage","id":"w1","parentId":"u1","timestamp":"2026-10-08T00:00:02.000Z","#,
    r#""kind":"cache_warm","provider":"anthropic","model":"claude-sonnet-5","#,
    r#""usage":{"input":0,"output":1,"cacheRead":42000,"cacheWrite":0,"totalTokens":42001,"#,
    r#""cost":{"input":0,"output":0.000015,"cacheRead":0.0126,"cacheWrite":0,"total":0.012615}},"#,
    r#""note":"extension override"}"#,
);

/// The same row WITHOUT a note. pi spreads the key in conditionally — `...(note ? { note } : {})`
/// — so it is ABSENT, never `null`, and never the empty string.
const USAGE_ROW_NO_NOTE: &str = concat!(
    r#"{"type":"usage","id":"w2","parentId":"w1","timestamp":"2026-10-08T00:00:03.000Z","#,
    r#""kind":"cache_warm","provider":"anthropic","model":"claude-sonnet-5","#,
    r#""usage":{"input":0,"output":1,"cacheRead":42000,"cacheWrite":0,"totalTokens":42001,"#,
    r#""cost":{"input":0,"output":0.000015,"cacheRead":0.0126,"cacheWrite":0,"total":0.012615}}}"#,
);

const USER_ROW: &str = r#"{"type":"message","id":"u1","parentId":null,"timestamp":"2026-10-08T00:00:01.000Z","message":{"role":"user","content":[{"type":"text","text":"hello"}],"timestamp":1759881601000}}"#;
const USER_ROW_2: &str = r#"{"type":"message","id":"u2","parentId":"w2","timestamp":"2026-10-08T00:00:04.000Z","message":{"role":"user","content":[{"type":"text","text":"again"}],"timestamp":1759881604000}}"#;

fn user_text(s: &str) -> Message {
    Message::User {
        content: vec![Content::text(s)],
        timestamp: 0,
    }
}

fn pi_file(lines: &[&str]) -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("pi.jsonl");
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    (tmp, path)
}

/// A `usage` row pi wrote must come back as the very bytes pi wrote — key order and all. A parsed
/// comparison would pass whatever the order is, and the order is the only thing a rewrite can
/// change.
///
/// **Red-proved** three ways, each a mutant a reviewer could plausibly introduce — every one of
/// them leaves the PROMOTION test below green, which is why byte order needs its own assertion:
/// * declaring the variant's fields with `note` BEFORE `usage` → this test FAILED;
/// * dropping `skip_serializing_if` on `note` → this test and
///   `sess051_append_usage_returns_the_entry_and_advances_the_leaf` both FAILED (an absent note
///   becomes `"note":null`);
/// * moving the flattened `base` to the END of the variant → this test FAILED
///   (`id`/`parentId`/`timestamp` land after the payload instead of before it).
#[test]
fn sess051_a_pi_usage_row_rewrites_byte_for_byte() {
    for row in [USAGE_ROW_WITH_NOTE, USAGE_ROW_NO_NOTE] {
        let entry: Entry = serde_json::from_str(row).unwrap();
        assert_eq!(serde_json::to_string(&entry).unwrap(), row);
    }

    // And through a whole-file load + branch rewrite, which is the path a fork or a migration takes.
    let (_tmp, path) = pi_file(&[HEADER, USER_ROW, USAGE_ROW_WITH_NOTE, USAGE_ROW_NO_NOTE]);
    let mgr = SessionManager::open(&path).unwrap();
    let mut written_bytes: Vec<u8> = Vec::new();
    mgr.export_branch_jsonl(&mut written_bytes).unwrap();
    let written = String::from_utf8(written_bytes).unwrap();
    assert!(
        written.contains(USAGE_ROW_WITH_NOTE),
        "noted usage row changed on rewrite:\n{written}"
    );
    assert!(
        written.contains(USAGE_ROW_NO_NOTE),
        "note-less usage row changed on rewrite:\n{written}"
    );
}

/// The row must PROMOTE to the typed variant. The variant alone is dead weight: `Entry`'s
/// `Deserialize` only parses strictly when the tag is in `KNOWN_TYPES`, so without the tag the row
/// still lands in `Entry::Unknown` — where it round-trips faithfully but is invisible to stats, the
/// cost breakdown, the cache-waste scan and the transcript.
///
/// **Red-proved** by removing `"usage"` from `KNOWN_TYPES` (leaving the variant in place): this test
/// failed with `Entry::Unknown`, while `sess051_a_pi_usage_row_rewrites_byte_for_byte` kept
/// passing — which is exactly why the promotion needs its own assertion.
#[test]
fn sess051_a_pi_usage_row_promotes_to_the_known_variant() {
    let entry: Entry = serde_json::from_str(USAGE_ROW_WITH_NOTE).unwrap();
    let Entry::Known(KnownEntry::Usage {
        kind,
        provider,
        model,
        usage,
        note,
        base,
    }) = entry
    else {
        panic!("a `usage` row must not demote to Entry::Unknown: {entry:?}");
    };
    assert_eq!(kind, "cache_warm");
    assert_eq!(provider.as_str(), "anthropic");
    assert_eq!(model.as_str(), "claude-sonnet-5");
    assert_eq!(note.as_deref(), Some("extension override"));
    assert_eq!(usage.cache_read, 42_000);
    assert_eq!(usage.output, 1);
    assert!((usage.cost.total - 0.012_615).abs() < 1e-12);
    assert_eq!(base.parent_id.as_ref().map(|p| p.as_str()), Some("u1"));

    // A row whose body does NOT fit the variant keeps the lossless fallback (`entry.rs`'s
    // `Err(_) => Entry::Unknown(v)`), which is what the `/tree` filter's surviving Unknown guard is
    // for.
    let malformed =
        r#"{"type":"usage","id":"x1","parentId":null,"timestamp":"t","kind":"cache_warm"}"#;
    assert!(matches!(
        serde_json::from_str::<Entry>(malformed).unwrap(),
        Entry::Unknown(_)
    ));
}

/// `append_usage` writes pi's bytes, returns the ENTRY (not its id, unlike every sibling), and
/// ADVANCES the leaf — pi's `_appendEntry` sets `this.leafId = entry.id` for every entry type
/// (`session-manager.ts:1191-1196`), so the next message parents to the warm.
///
/// **Red-proved** twice, and in both cases this was the ONLY test that failed:
/// * restoring `self.leaf` after `push_entry` (a leaf-preserving write, which is what survey 2 of
///   this effort wrongly prescribed — pi's `_appendEntry` advances it, verified at v1.0.4) → FAILED;
/// * dropping the `note.filter(|n| !n.is_empty())` normalization → FAILED, because `Some("")`
///   serializes as `"note":""`, a key pi's `...(note ? { note } : {})` never writes.
#[test]
fn sess051_append_usage_returns_the_entry_and_advances_the_leaf() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cwd = tmp.path().join("work");
    std::fs::create_dir_all(&cwd).unwrap();
    let layout = crate::SessionLayout::new(tmp.path().to_path_buf(), cwd.clone());
    let mut mgr = SessionManager::create(&cwd, &layout, crate::NewSessionOpts::default()).unwrap();
    let first = mgr.append_message(user_text("hello")).unwrap();

    let usage = Usage {
        input: 0,
        output: 1,
        cache_read: 42_000,
        total_tokens: 42_001,
        ..Usage::default()
    };
    let entry = mgr
        .append_usage(
            "cache_warm",
            "anthropic".into(),
            "claude-sonnet-5".into(),
            usage.clone(),
            Some("extension override"),
        )
        .unwrap();

    let Entry::Known(KnownEntry::Usage { base, note, .. }) = &entry else {
        panic!("append_usage must return the typed entry: {entry:?}");
    };
    assert_eq!(note.as_deref(), Some("extension override"));
    assert_eq!(base.parent_id.as_ref(), Some(&first));
    let warm_id = base.id.clone();

    // pi's `_appendEntry` advances the leaf for EVERY entry, so the next message hangs off the warm.
    assert_eq!(mgr.leaf_id(), Some(&warm_id));
    let next = mgr.append_message(user_text("again")).unwrap();
    assert_eq!(
        mgr.entry(&next).and_then(Entry::parent_id).as_ref(),
        Some(&warm_id),
        "the entry after a warm must parent to the warm"
    );

    // An empty note is pi-falsy: ABSENT on the wire, not `""` and not `null`.
    let bare = mgr
        .append_usage(
            "cache_warm",
            "anthropic".into(),
            "claude-sonnet-5".into(),
            usage,
            Some(""),
        )
        .unwrap();
    let json = serde_json::to_string(&bare).unwrap();
    assert!(
        !json.contains("note"),
        "empty note must not be written: {json}"
    );
}

/// The test the silent catch-alls need. A usage entry sits between two user messages and must:
///
/// * project ZERO context messages (pi's `sessionEntryToContextMessages` has no `usage` arm and
///   falls through to `return []`, `session-manager.ts:439-465`);
/// * change the context-token estimate by 0;
/// * be FOLDED PAST by the compaction cut-point back-scan, which stops only at a compaction or a
///   context-VISIBLE entry (`compaction.ts:464-470`).
///
/// **Red-proved** two ways, each failing this test alone:
/// * giving `context::push_as_raw` a `KnownEntry::Usage` arm that pushes a custom message → FAILED
///   (the warm became a context message and moved the token estimate);
/// * giving `context_message_role` a `Some(MessageRole::User)` arm for it — the predicate
///   `compaction::cutpoint`'s `is_context_visible` reads, so this is what would make the back-scan
///   stop AT the warm instead of folding past it → FAILED.
#[test]
fn sess051_usage_entry_is_context_invisible_and_folded_by_the_back_scan() {
    let entry: Entry = serde_json::from_str(USAGE_ROW_WITH_NOTE).unwrap();
    assert!(
        crate::context::raw_context_messages(&entry).is_empty(),
        "a usage entry projects no context message"
    );
    assert!(
        crate::context::context_message_role(&entry).is_none(),
        "a usage entry has no context role — `is_context_visible` reads this"
    );

    // Same session with and without the warm: identical projection, identical token estimate.
    let with = [HEADER, USER_ROW, USAGE_ROW_WITH_NOTE, USER_ROW_2];
    let without = [HEADER, USER_ROW, USER_ROW_2];
    let (_t1, p1) = pi_file(&with);
    let (_t2, p2) = pi_file(&without);
    let m1 = SessionManager::open(&p1).unwrap();
    let m2 = SessionManager::open(&p2).unwrap();
    let c1 = m1.build_context_raw();
    let c2 = m2.build_context_raw();
    assert_eq!(c1.len(), c2.len(), "the warm added a context message");
    assert_eq!(
        crate::compaction::tokens::estimate_context_tokens_raw(&c1).tokens,
        crate::compaction::tokens::estimate_context_tokens_raw(&c2).tokens,
        "the warm changed the compaction token estimate"
    );

    // The back-scan must fold the warm into the KEPT region: with the cut landing on `u2`, the
    // bookkeeping entry in front of it comes along, so nothing context-visible is re-admitted.
    let entries: Vec<Entry> = with[1..]
        .iter()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let roles: Vec<_> = entries
        .iter()
        .map(crate::context::context_message_role)
        .collect();
    assert_eq!(
        roles,
        vec![Some(MessageRole::User), None, Some(MessageRole::User)],
        "the middle entry is the usage row and must be the invisible one"
    );
}

/// A usage entry contributes its tokens to nothing in `cyrup-session` itself — it is the readers in
/// `cyrup-session-svc` that count it. Pinned here so the content assertion above cannot be read as
/// "a warm is free": the `Content` import keeps the user rows honest.
#[test]
fn sess051_usage_row_carries_no_content() {
    let entry: Entry = serde_json::from_str(USAGE_ROW_NO_NOTE).unwrap();
    let blocks: Vec<Content> = crate::context::raw_context_messages(&entry)
        .into_iter()
        .filter_map(|m| match m {
            crate::agent_message::AgentMessage::Core(Message::User { content, .. }) => {
                Some(content)
            }
            _ => None,
        })
        .flatten()
        .collect();
    assert!(blocks.is_empty());
}
