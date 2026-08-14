//! Regression tests for the 2026-08-14 area-03 (`docs/gap-analysis/03-cyrup-session.md`) pass.
//!
//! Every test here would have been RED before the change it names and green after. Upstream
//! citations are `pi` @ **v0.83.0** (cyrup's ported baseline), taken from
//! `git show v0.83.0:packages/coding-agent/src/<path>`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::path::{Path, PathBuf};

use cyrup_core::{AssistantMessage, Content, Message, StopReason, Usage};
use serde_json::json;

use crate::agent_message::custom_to_message;
use crate::compaction::serialize::serialize_conversation;
use crate::compaction::tokens::estimate_tokens;
use crate::entry::{Entry, KnownEntry};
use crate::layout::encode_cwd;
use crate::{NewSessionOpts, SessionManager};

fn user(s: &str) -> Message {
    Message::User { content: vec![Content::text(s)], timestamp: 0 }
}

fn assistant_blocks(content: Vec<Content>) -> Message {
    Message::Assistant(AssistantMessage {
        content,
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

fn image_block() -> Content {
    // Any image block; only the discriminant matters to the estimator.
    serde_json::from_value::<Content>(json!({
        "type": "image",
        "data": "AAAA",
        "mimeType": "image/png"
    }))
    .expect("image content block")
}

// ── SESS-007 / SESS-014 — first PARSED entry is the header, not the first physical line ─────────
//
// Pi `parseSessionEntryLine` returns null for a blank AND for a malformed line
// (`session-manager.ts:503-511`); `loadEntriesFromFile` validates `entries[0]` only after those
// have been dropped (`:548-553`). A stray leading newline therefore opens fine upstream.

fn write_session_with_prefix(dir: &Path, prefix: &str) -> PathBuf {
    let path = dir.join("s.jsonl");
    let header = json!({
        "type": "session",
        "version": crate::header::CURRENT_VERSION,
        "id": "01234567-89ab-cdef-0123-456789abcdef",
        "timestamp": "2026-01-01T00:00:00.000Z",
        "cwd": "/proj/x",
    });
    std::fs::write(&path, format!("{prefix}{header}\n")).expect("write fixture");
    path
}

#[test]
fn sess007_leading_blank_line_still_opens_and_listing_agrees() {
    let tmp = tempfile::tempdir().unwrap();

    // A single stray leading newline: previously `lineno == 0` was the blank line, the real header
    // landed at `lineno == 1` and was parsed as an ordinary entry, so `load` returned NotASession.
    let path = write_session_with_prefix(tmp.path(), "\n");
    let mgr = SessionManager::open(&path).expect("a leading blank line must not break `open`");
    assert_eq!(mgr.cwd(), Path::new("/proj/x"), "header cwd survives the blank prefix");

    // The listing side must agree with the loader — before this change `read_header` skipped blanks
    // while `scan_file` took `lines.next()` unconditionally, so the two disagreed about whether the
    // file was a session at all.
    let infos = crate::listing::list_in_dir(tmp.path(), None, None);
    assert_eq!(infos.len(), 1, "scan_file uses the same first-parsed-entry rule");
    assert_eq!(infos[0].cwd, "/proj/x");
    assert_eq!(
        crate::listing::newest_session(tmp.path(), None).as_deref(),
        Some(path.as_path()),
        "read_header uses the same rule"
    );

    // Several blanks plus a malformed line: pi skips both classes and keeps scanning.
    let path2 = write_session_with_prefix(tmp.path(), "\n\n   \nnot json at all\n");
    let mgr2 = SessionManager::open(&path2).expect("blank + malformed prefix still opens");
    assert_eq!(mgr2.cwd(), Path::new("/proj/x"));
}

#[test]
fn sess007_non_empty_file_that_is_not_a_session_is_still_an_error() {
    // Pi does NOT soft-empty this case: `loadEntriesFromFile` returns `[]` and `_setSessionFile`
    // then throws `Session file is not a valid pi session: <path>` because `statSync(path).size > 0`
    // (`session-manager.ts:900-906`). Only a missing or zero-length file is a fresh session.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("notes.jsonl");
    std::fs::write(&path, "{\"type\":\"message\",\"id\":\"aa\"}\n").unwrap();
    match SessionManager::open(&path) {
        Err(crate::SessionError::NotASession { .. }) => {}
        Err(other) => panic!("expected NotASession, got {other:?}"),
        Ok(_) => panic!("a non-session first entry must not be soft-emptied"),
    }
}

// ── SESS-037 — a missing/zero-length target must not write `"cwd": ""` ──────────────────────────
//
// Pi `static open`: `const cwd = cwdOverride ?? (header ? getSessionHeaderCwd(header) : undefined)
// ?? process.cwd();` (`session-manager.ts:1546`), written into the header by `newSession`
// (`:941`). `PathBuf::default()` is the EMPTY path, and an empty header cwd can never satisfy
// `session_cwd_matches` (`:630-632`), so such a session vanished from every cwd-filtered listing.
#[test]
fn sess037_missing_or_empty_target_records_a_real_cwd() {
    let tmp = tempfile::tempdir().unwrap();
    let process_cwd = std::env::current_dir().expect("a process cwd");

    for (name, pre_create) in [("missing.jsonl", false), ("empty.jsonl", true)] {
        let path = tmp.path().join(name);
        if pre_create {
            std::fs::write(&path, "").unwrap();
        }
        let mut mgr = SessionManager::open(&path).expect("anchors a new session at this path");
        assert_eq!(mgr.cwd(), process_cwd.as_path(), "{name}: falls back to process.cwd()");

        // Flush so the header reaches disk (the first ASSISTANT message is what creates the file —
        // pi's deferred `_persist`), then read it back: the recorded cwd must be non-empty and must
        // make the session visible to a cwd-filtered listing.
        mgr.append_message(user("hello")).unwrap();
        mgr.append_message(assistant_blocks(vec![Content::text("hi")])).unwrap();
        let text = std::fs::read_to_string(&path).expect("flushed");
        let header: serde_json::Value =
            serde_json::from_str(text.lines().next().expect("header line")).unwrap();
        assert_eq!(header["cwd"], json!(process_cwd.to_string_lossy()), "{name}");
        assert!(
            !crate::listing::list_in_dir(tmp.path(), Some(&process_cwd), None).is_empty(),
            "{name}: a cwd-filtered listing must find it"
        );
    }
}

// ── SESS-044 — `encode_cwd` strips EXACTLY ONE leading separator ────────────────────────────────
//
// Pi `session-manager.ts:479` (byte-identical to `migrations.ts:112`):
// ``--${resolvedCwd.replace(/^[/\\]/, "").replace(/[/\\:]/g, "-")}--``. The first `replace` is
// anchored with NO `g` flag.
#[test]
fn sess044_encode_cwd_strips_exactly_one_leading_separator() {
    assert_eq!(encode_cwd(Path::new("/Users/x/proj")), "--Users-x-proj--", "single slash");
    assert_eq!(encode_cwd(Path::new("//net/x")), "---net-x--", "second slash becomes a dash");
    assert_eq!(encode_cwd(Path::new(r"\\srv\share\proj")), "---srv-share-proj--", "UNC");
    assert_eq!(encode_cwd(Path::new("C:/a/b")), "--C--a-b--", "drive colon still mapped");
    assert_eq!(encode_cwd(Path::new("rel/path")), "--rel-path--", "no leading separator to strip");
}

// ── SESS-018 — `custom_message` null content and a missing `display` ─────────────────────────────
//
// Pi `createCustomMessage(entry.customType, entry.content ?? [], entry.display, …)`
// (`session-manager.ts:396-399`); `createCustomMessage` itself validates nothing
// (`messages.ts:123-136`).
#[test]
fn sess018_custom_message_null_content_and_absent_display() {
    // (a) null content contributes ZERO blocks, not the four characters `null`.
    let msg = custom_to_message(&serde_json::Value::Null, 0);
    match &msg {
        Message::User { content, .. } => {
            assert!(content.is_empty(), "null normalizes to [], got {content:?}");
        }
        other => panic!("expected a user message, got {other:?}"),
    }

    // (b) an entry with no `display` key survives as a typed CustomMessage with `display == false`,
    // instead of being demoted to `Entry::Unknown` and vanishing from context.
    let line = json!({
        "type": "custom_message",
        "id": "aabbccdd",
        "parentId": null,
        "timestamp": "2026-01-01T00:00:00.000Z",
        "customType": "note",
        "content": null,
    })
    .to_string();
    let entry: Entry = serde_json::from_str(&line).expect("parses");
    match entry {
        Entry::Known(KnownEntry::CustomMessage { display, content, .. }) => {
            assert!(!display, "absent `display` is falsy upstream");
            assert!(content.is_null());
        }
        other => panic!("expected a typed CustomMessage, got {other:?}"),
    }
}

// ── SESS-026 — `[Assistant]: ` is emitted for an EMPTY text block ────────────────────────────────
//
// Pi guards on presence: `if (msg.content.some((block) => block.type === "text"))`
// (`compaction/utils.ts:135-137`); the user and toolResult arms guard on emptiness instead.
#[test]
fn sess026_assistant_marker_survives_an_empty_text_block() {
    let out = serialize_conversation(&[
        user("question"),
        assistant_blocks(vec![Content::text("")]),
        user("follow up"),
    ]);
    assert!(out.contains("[Assistant]: "), "marker present for an empty text block; got:\n{out}");
    assert_eq!(
        out, "[User]: question\n\n[Assistant]: \n\n[User]: follow up",
        "the empty assistant turn keeps its slot in the interleaving"
    );

    // An assistant turn with NO text block at all still emits nothing (pi's `.some()` is false).
    let no_text = serialize_conversation(&[assistant_blocks(vec![])]);
    assert!(!no_text.contains("[Assistant]:"), "no text block => no marker; got:\n{no_text}");
}

// ── SESS-029 — role-dispatched content estimation ────────────────────────────────────────────────
//
// Pi `estimateTokens` (`compaction/compaction.ts:266-301`): the assistant arm (`:276-288`) counts
// text/thinking/toolCall and NOT image; `estimateTextAndImageContentChars` (`:246-260`), used by
// the user and toolResult arms, counts text and image and NOT thinking/toolCall.
#[test]
fn sess029_estimate_tokens_dispatches_on_role() {
    let text = "abcd".repeat(10); // 40 chars => 10 tokens
    let want = 10u32;

    // Assistant: an image block costs NOTHING.
    let a = assistant_blocks(vec![Content::text(&text), image_block()]);
    assert_eq!(estimate_tokens(&a), want, "an assistant image contributes 0 (compaction.ts:276-288)");

    // User: an image block DOES cost ESTIMATED_IMAGE_CHARS/4 = 1200.
    let u = Message::User { content: vec![Content::text(&text), image_block()], timestamp: 0 };
    assert_eq!(estimate_tokens(&u), want + 1200, "a user image costs 4800 chars (compaction.ts:250)");

    // User: a thinking block contributes NOTHING (it falls through pi's if/else-if chain).
    let thinking = serde_json::from_value::<Content>(json!({
        "type": "thinking",
        "thinking": "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
    }))
    .expect("thinking block");
    let u2 = Message::User { content: vec![Content::text(&text), thinking.clone()], timestamp: 0 };
    assert_eq!(estimate_tokens(&u2), want, "a user thinking block contributes 0");

    // Assistant: a thinking block DOES count.
    let a2 = assistant_blocks(vec![Content::text(&text), thinking]);
    assert_eq!(estimate_tokens(&a2), want + 10, "an assistant thinking block counts");
}

// ── SESS-S05 — `TreeNode` carries pi's `labelTimestamp` ─────────────────────────────────────────
//
// Pi `SessionTreeNode { entry; children; label?; labelTimestamp? }` (`session-manager.ts:159-167`).
#[test]
fn sess_s05_tree_node_exposes_the_label_timestamp() {
    let cwd = PathBuf::from("/proj/labels");
    let mut m = SessionManager::in_memory(&cwd, NewSessionOpts::default()).unwrap();
    let root = m.append_message(user("q")).unwrap();
    m.append_label(&root, Some("bookmark")).unwrap();

    let tree = m.tree();
    let node = tree.first().expect("one root");
    assert_eq!(node.label.as_deref(), Some("bookmark"));
    let ts = node.label_timestamp.as_deref().expect("labelTimestamp is produced");
    assert!(!ts.is_empty(), "the manager already held it; getTree must hand it out");
    assert_eq!(Some(ts), m.label_timestamp(&root));

    // Clearing the label clears both halves.
    m.append_label(&root, None).unwrap();
    let cleared = m.tree();
    let node = cleared.first().expect("one root");
    assert!(node.label.is_none() && node.label_timestamp.is_none());
}

// ── SESS-036 — the ancestor walk is LEXICAL, like pi's `resolvePath` ────────────────────────────
//
// Pi `const resolvedCwd = resolvePath(options.cwd);` (`resource-loader.ts:122`) — `node:path.resolve`
// (`utils/paths.ts:81-85`), which does NOT follow symlinks. cyrup canonicalized instead, so a cwd
// arriving from a non-`getcwd` source (`--cwd`, a resumed header, a `cwd_override`) walked the
// target's ancestors where pi walks the link's.
#[cfg(unix)]
#[test]
fn sess036_ancestor_walk_keeps_the_symlinked_path() {
    use crate::prompt::context_files::ContextFileLoader;

    let tmp = tempfile::tempdir().unwrap();
    let real = tmp.path().join("real");
    let proj = real.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(real.join("AGENTS.md"), "REAL_ANCESTOR").unwrap();
    let link = tmp.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let global = tmp.path().join("global-agent");
    std::fs::create_dir_all(&global).unwrap();

    let (files, _diags) = ContextFileLoader::new(link.join("proj"), global, true, false).load();
    let paths: Vec<&Path> = files.iter().map(|f| f.path.as_path()).collect();
    assert_eq!(
        paths,
        vec![link.join("AGENTS.md").as_path()],
        "pi reports the path the user supplied, not its realpath"
    );
}

// ── SESS-013 — a nested linked worktree does not load the main repo's AGENTS.md twice ───────────
//
// Pi `findShadowedContextFile` (`resource-loader.ts:100-116`) + the `isShadowed` gate at `:140-142`.
// The fixture is a hand-built worktree layout (no `git` invocation): `main/.git/` with a HEAD, and
// `main/wt/.git` a FILE pointing at `main/.git/worktrees/wt`, whose `commondir` names `main/.git`.
#[test]
fn sess013_nested_linked_worktree_loads_agents_md_once() {
    use crate::prompt::context_files::ContextFileLoader;

    let tmp = tempfile::tempdir().unwrap();
    let main = tmp.path().join("main");
    let git = main.join(".git");
    let wt = main.join("wt");
    let wt_gitdir = git.join("worktrees").join("wt");
    std::fs::create_dir_all(&wt_gitdir).unwrap();
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    std::fs::write(wt_gitdir.join("HEAD"), "ref: refs/heads/feat\n").unwrap();
    // `git worktree add` writes these in REALPATH form, which is why the predicate canonicalizes.
    std::fs::write(wt_gitdir.join("commondir"), "../..\n").unwrap();
    std::fs::write(
        wt.join(".git"),
        format!("gitdir: {}\n", wt_gitdir.canonicalize().unwrap().display()),
    )
    .unwrap();

    // The SAME tracked file, checked out in both trees.
    std::fs::write(main.join("AGENTS.md"), "TRACKED RULES").unwrap();
    std::fs::write(wt.join("AGENTS.md"), "TRACKED RULES").unwrap();

    let global = tmp.path().join("global-agent");
    std::fs::create_dir_all(&global).unwrap();

    let (files, _diags) = ContextFileLoader::new(wt.clone(), global.clone(), true, false).load();
    let contents: Vec<&str> = files.iter().map(|f| &*f.content).collect();
    assert_eq!(contents, vec!["TRACKED RULES"], "loaded once, not twice; got {files:?}");
    assert!(
        files[0].path.starts_with(&wt),
        "the worktree's own copy is the one kept (the ancestor is shadowed)"
    );

    // Control: an ORDINARY repo (no worktree) still inherits its ancestor's file, i.e. the
    // predicate returns `undefined` where `worktreeRoot === mainRepoRoot`.
    let plain = tmp.path().join("plain");
    let sub = plain.join("sub");
    std::fs::create_dir_all(plain.join(".git")).unwrap();
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(plain.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();
    std::fs::write(plain.join("AGENTS.md"), "PLAIN RULES").unwrap();
    let (plain_files, _) = ContextFileLoader::new(sub, global, true, false).load();
    assert_eq!(
        plain_files.iter().map(|f| &*f.content).collect::<Vec<_>>(),
        vec!["PLAIN RULES"],
        "ordinary ancestor inheritance is untouched"
    );
}

// ── SESS-004 — unknown top-level keys survive a rewrite ─────────────────────────────────────────
//
// Pi `parseSessionEntryLine` is a bare `JSON.parse` (`session-manager.ts:503-511`), so unknown keys
// survive any rewrite. cyrup typed the known variants and dropped them.
#[test]
fn sess004_unknown_top_level_keys_round_trip() {
    let line = json!({
        "type": "compaction",
        "id": "aabbccdd",
        "parentId": "11223344",
        "timestamp": "2026-01-01T00:00:00.000Z",
        "summary": "s",
        "firstKeptEntryId": "55667788",
        "tokensBefore": 42,
        "xExtensionAnnotation": {"by": "acme", "n": 7},
    });
    let entry: Entry = serde_json::from_value(line.clone()).expect("parses as a KNOWN entry");
    assert!(
        matches!(entry, Entry::Known(KnownEntry::Compaction { .. })),
        "an unknown key must not demote the entry to Unknown"
    );
    let back: serde_json::Value =
        serde_json::from_str(&entry.to_line().expect("serializes")).unwrap();
    assert_eq!(
        back["xExtensionAnnotation"], line["xExtensionAnnotation"],
        "the annotation survives the rewrite; got {back}"
    );
    assert_eq!(back["type"], json!("compaction"), "the tag is emitted exactly once");
}

// ── SESS-017 / SESS-022 / SESS-034 / SESS-042 live in `tests/compaction.rs` beside the other
//    `Compactor` coverage, where the summarizer and hook doubles already exist.

// ═══════════════════════════════════════════════════════════════════════════════════════════════
// 2026-08-14 second pass — items the area file recorded as candidates rather than findings, plus
// the SESS-014 assertion its Verify clause asked for and never got.
// ═══════════════════════════════════════════════════════════════════════════════════════════════

/// `computeFileLists` (`compaction/utils.ts:62-67` @v0.83.0) sorts with a bare
/// `Array.prototype.sort()`, i.e. by UTF-16 code units. cyrup collected from a `BTreeSet`, i.e. by
/// UTF-8 bytes. The two relations disagree whenever one path carries an astral code point (encoded
/// in UTF-16 as a surrogate pair beginning at `0xD800`) and the other a code point in
/// `U+E000..=U+FFFF`: UTF-16 puts the astral path FIRST, UTF-8 puts it LAST.
///
/// The order is not internal — the lists are joined into the `<read-files>` / `<modified-files>`
/// blocks appended to the persisted summary (`formatFileOperations`, `utils.ts:72-82`).
#[test]
fn compute_file_lists_sorts_by_utf16_code_units_like_pi() {
    use crate::compaction::files::{format_file_operations, FileOps};

    // U+1F600 GRINNING FACE — UTF-16 `D83D DE00`, UTF-8 `F0 9F 98 80`.
    let astral = "/p/\u{1F600}.rs";
    // U+E000 (private use) — UTF-16 `E000`, UTF-8 `EE 80 80`. `0xD83D < 0xE000` but `0xF0 > 0xEE`.
    let bmp = "/p/\u{E000}.rs";
    assert!(astral > bmp, "precondition: Rust byte order puts the astral path LAST");

    let mut ops = FileOps::default();
    ops.read.insert(bmp.to_string());
    ops.read.insert(astral.to_string());
    ops.edited.insert(format!("{bmp}x"));
    ops.written.insert(format!("{astral}x"));

    let (read, modified) = ops.compute_lists();
    assert_eq!(
        read,
        vec![astral.to_string(), bmp.to_string()],
        "Pi's `.sort()` orders the surrogate pair before U+E000"
    );
    assert_eq!(
        modified,
        vec![format!("{astral}x"), format!("{bmp}x")],
        "the modified list takes the same comparator"
    );

    let block = format_file_operations(&read, &modified);
    let read_body = block
        .split("<read-files>\n")
        .nth(1)
        .and_then(|s| s.split("\n</read-files>").next())
        .expect("read block present");
    assert_eq!(
        read_body,
        format!("{astral}\n{bmp}"),
        "the order reaches the persisted summary text, which is what makes it observable"
    );
}

/// `generateSummaryWithUsage` (`compaction.ts:637-640` @v0.83.0) and `generateTurnPrefixSummary`
/// (`:937-940`) are `Math.min(Math.floor(frac * reserveTokens), maxTokens > 0 ? maxTokens : ∞)`
/// with **no lower bound**. cyrup carried a `.max(1)` clamp that has no upstream counterpart, so a
/// small `CompactionSettings.reserve_tokens` asked for one token where pi asks for zero.
#[test]
fn summarization_max_tokens_has_no_lower_clamp() {
    use crate::compaction::summarize::compute_max_tokens_frac;

    // 0.8 fraction: floor(0.8 * 1) == 0 upstream.
    assert_eq!(compute_max_tokens_frac(1, 0, 4, 5), 0, "unbounded model, floor is 0 not 1");
    assert_eq!(compute_max_tokens_frac(1, 4096, 4, 5), 0, "bounded model, floor is 0 not 1");
    // 0.5 fraction (turn prefix): floor(0.5 * 1) == 0.
    assert_eq!(compute_max_tokens_frac(1, 4096, 1, 2), 0, "turn-prefix half takes the same rule");
    assert_eq!(compute_max_tokens_frac(0, 4096, 4, 5), 0, "a zero reserve stays zero");
    // The ordinary path is unchanged.
    assert_eq!(compute_max_tokens_frac(16384, 0, 4, 5), 13107, "floor(0.8 * 16384)");
    assert_eq!(compute_max_tokens_frac(16384, 4096, 4, 5), 4096, "model cap wins");
    assert_eq!(compute_max_tokens_frac(16384, 0, 1, 2), 8192, "floor(0.5 * 16384)");
}

/// SESS-014's Verify clause — "point `read_header` at a multi-megabyte session and assert bytes
/// read stay under the cap" — was never asserted when the bounded reader landed.
///
/// The cap is only observable as a difference in what the two readers FIND, so this drives both:
/// pi's discovery path (`findMostRecentSession` → `readSessionHeaderForDiscovery` →
/// `readSessionHeader`, `session-manager.ts:571-613`) gives up after
/// `MAX_SESSION_HEADER_SCAN_BYTES` (1 MiB) and reports "not a session", while pi's listing path
/// (`buildSessionInfo`, `:696-736`) streams the whole file and finds the header regardless. An
/// unbounded `read_header` — cyrup's `read_to_string` before the fix — would find it in both.
#[test]
fn sess014_header_discovery_stops_at_pis_one_mebibyte_scan_cap() {
    use std::io::Write as _;

    let dir = tempfile::tempdir().unwrap();
    let header = json!({
        "type": "session",
        "version": 3,
        "id": "0193f0e1-0000-7000-8000-0000000000aa",
        "timestamp": "2026-01-01T00:00:00.000Z",
        "cwd": "/proj/capped",
    })
    .to_string();

    // A skippable prefix: each line is non-blank and unparseable, so BOTH readers keep scanning.
    // 1.5 MiB of it puts the real header past the 1 MiB cap.
    let junk = "not json at all — keep scanning\n";
    let prefix_bytes = 1_536 * 1024;

    let capped = dir.path().join("capped.jsonl");
    {
        let mut f = std::fs::File::create(&capped).unwrap();
        let mut written = 0usize;
        while written < prefix_bytes {
            f.write_all(junk.as_bytes()).unwrap();
            written += junk.len();
        }
        f.write_all(header.as_bytes()).unwrap();
        f.write_all(b"\n").unwrap();
    }

    assert_eq!(
        crate::listing::newest_session(dir.path(), None),
        None,
        "the header sits past the 1 MiB cap, so bounded discovery must not find it"
    );

    let infos = crate::listing::list_in_dir(dir.path(), None, None);
    assert_eq!(infos.len(), 1, "the unbounded listing scan still reads the file: {infos:?}");
    assert_eq!(infos[0].cwd, "/proj/capped", "…and it is the same header the cap hid");

    // Control: the identical header inside the cap IS discovered, so the assertion above is about
    // the byte bound and not about the junk prefix being unreadable.
    let dir2 = tempfile::tempdir().unwrap();
    let small = dir2.path().join("small.jsonl");
    std::fs::write(&small, format!("{junk}{junk}{header}\n")).unwrap();
    assert_eq!(
        crate::listing::newest_session(dir2.path(), None),
        Some(small),
        "a header within the cap is still found after skippable lines"
    );
}

/// A header missing (or carrying a non-string) `cwd` / `timestamp` must still be a session.
///
/// pi's `interface SessionHeader` (`session-manager.ts:32-39` @v0.83.0) declares both as required,
/// but that type is erased at runtime: the two header validators check `type` and `id` only
/// (`:548-552`, `:566`), and every consumer re-checks the other fields by hand —
/// `getSessionHeaderCwd` (`:625-628`) and `buildSessionInfo` (`:739`, `:742`). cyrup declared them
/// as required `String`s, so serde rejected the line, `load` returned `NotASession` and the file
/// disappeared from listings — the same TypeScript-is-compile-time-only mechanism gap as
/// `SESS-001`.
#[test]
fn a_header_without_cwd_or_timestamp_still_opens_like_pi() {
    let dir = tempfile::tempdir().unwrap();

    for (label, header) in [
        (
            "absent",
            json!({"type": "session", "version": 3, "id": "0193f0e1-0000-7000-8000-00000000000b"}),
        ),
        (
            "null",
            json!({
                "type": "session", "version": 3,
                "id": "0193f0e1-0000-7000-8000-00000000000c",
                "cwd": null, "timestamp": null,
            }),
        ),
        (
            "wrong type",
            json!({
                "type": "session", "version": 3,
                "id": "0193f0e1-0000-7000-8000-00000000000d",
                "cwd": 7, "timestamp": 7,
            }),
        ),
    ] {
        let sub = dir.path().join(label.replace(' ', "_"));
        std::fs::create_dir_all(&sub).unwrap();
        let path = sub.join("s.jsonl");
        std::fs::write(&path, format!("{header}\n")).unwrap();

        let m = SessionManager::open(&path)
            .unwrap_or_else(|e| panic!("[{label}] pi opens this file; cyrup returned {e:?}"));
        assert_eq!(
            m.header().cwd,
            "",
            "[{label}] pi's `typeof cwd === \"string\" ? cwd : \"\"` lands on the empty string"
        );
        assert_ne!(
            m.cwd(),
            Path::new(""),
            "[{label}] and the manager cwd falls through to the process cwd, per \
             `cwdOverride ?? getSessionHeaderCwd(header) ?? process.cwd()` (`:1546`)"
        );

        // The listing side must agree with the loader — the SESS-007 invariant.
        let infos = crate::listing::list_in_dir(&sub, None, None);
        assert_eq!(infos.len(), 1, "[{label}] the file is still a listable session: {infos:?}");
        assert_eq!(infos[0].cwd, "", "[{label}] `buildSessionInfo`'s `: \"\"` arm");
        assert_eq!(
            crate::listing::newest_session(&sub, None),
            Some(path),
            "[{label}] and discovery finds it too"
        );
    }

    // The two checks pi DOES make on read are unchanged.
    let bad = dir.path().join("bad.jsonl");
    std::fs::write(&bad, "{\"type\":\"session\",\"id\":42}\n").unwrap();
    assert!(
        SessionManager::open(&bad).is_err(),
        "a non-string `id` is pi's `typeof header.id !== \"string\"` rejection"
    );
}

/// Pi's `migrateV1ToV2` guard is `typeof comp.firstKeptEntryIndex === "number"`
/// (`session-manager.ts:245-247` @v0.83.0), and the `delete comp.firstKeptEntryIndex` inside it
/// runs for EVERY number — negative and fractional included. cyrup matched on `as_u64`, so those
/// two shapes returned early and left the dead v1 key on the entry, which the migration rewrite
/// then persisted. `entries[1.0]` is also a hit upstream, because a JS property access stringifies
/// the index and `String(1.0) === "1"`.
#[test]
fn v1_first_kept_entry_index_drops_for_every_json_number() {
    use crate::header::SessionHeader;
    use crate::migrate::to_current;

    let build = |index: serde_json::Value| {
        let mut header = SessionHeader::new(
            cyrup_core::SessionId::from("sess-v1-nums"),
            "/proj",
            "2026-01-01T00:00:00Z",
        );
        header.version = None; // v1
        let msg = |ts: &str, body: &str| {
            serde_json::from_value::<Entry>(json!({
                "type": "message",
                "timestamp": ts,
                "message": serde_json::to_value(
                    crate::agent_message::AgentMessage::Core(user(body)),
                )
                .unwrap(),
            }))
            .unwrap()
        };
        let mut entries = vec![
            msg("2026-01-01T00:00:01Z", "first"),
            msg("2026-01-01T00:00:02Z", "kept"),
            serde_json::from_value::<Entry>(json!({
                "type": "compaction",
                "timestamp": "2026-01-01T00:00:03Z",
                "summary": "SUM",
                "tokensBefore": 42,
                "firstKeptEntryIndex": index,
            }))
            .unwrap(),
        ];
        assert!(to_current(&mut header, &mut entries));
        entries
    };

    for index in [json!(-1), json!(-1.5), json!(1.5), json!(2.5)] {
        let entries = build(index.clone());
        let line = entries[2].to_line().expect("serializes");
        assert!(
            !line.contains("firstKeptEntryIndex"),
            "pi deletes the key for any number, including {index}: {line}"
        );
        assert!(
            !line.contains("firstKeptEntryId"),
            "{index} resolves to no element upstream, so no id is assigned: {line}"
        );
    }

    // `2.0` stringifies to "2" for the property access, so it resolves exactly like the integer.
    let entries = build(json!(2.0));
    let kept_id = entries[1].id();
    match &entries[2] {
        Entry::Known(KnownEntry::Compaction { first_kept_entry_id, .. }) => assert_eq!(
            first_kept_entry_id.as_ref(),
            Some(&kept_id),
            "`entries[2.0]` is `entries[\"2\"]` upstream — the same element as `entries[2]`"
        ),
        other => panic!("expected a parsed compaction, got {other:?}"),
    }
}
