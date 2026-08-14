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
