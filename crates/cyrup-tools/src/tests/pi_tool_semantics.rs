//! Byte-diff regressions for the 2026-08-14 `04-cyrup-tools` pass.
//!
//! Every test here pins a property read out of `pi/packages/coding-agent/src/core/tools/**` at
//! **v0.84.1** and names the file:line it came from. Each was RED before the change it guards.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use crate::FileMutationLocks;
use crate::config::{EditOpts, FindOpts, GrepOpts, LsOpts, WriteOpts};
use crate::ops::local::LocalFs;
use crate::ops::{Access, DirEntry, FsOps, Meta, WalkItem, WalkOpts};
use crate::tools::{EditTool, FindTool, GrepTool, LsTool, WriteTool};
use cyrup_core::{
    CancelToken, Content, EventStream, ExecMode, Tool, ToolCallId, ToolError, ToolRenderKind,
    ToolResult, ToolUpdate, ToolUpdateSink,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn cid() -> ToolCallId {
    ToolCallId::from("tc-test")
}

fn noop_sink() -> ToolUpdateSink {
    Box::new(|_u: ToolUpdate| {})
}

fn first_text(r: &ToolResult) -> String {
    match r.content.first() {
        Some(Content::Text { text, .. }) => text.clone(),
        _ => String::new(),
    }
}

fn edit_tool(cwd: PathBuf) -> EditTool {
    EditTool::new(
        Arc::new(LocalFs),
        Arc::new(FileMutationLocks::new()),
        cwd,
        EditOpts,
    )
}

fn write_tool(cwd: PathBuf) -> WriteTool {
    WriteTool::new(
        Arc::new(LocalFs),
        Arc::new(FileMutationLocks::new()),
        cwd,
        WriteOpts,
    )
}

// ---------------------------------------------------------------------------------------------
// TOOL-006 — neither mutator may declare `Sequential`.
// ---------------------------------------------------------------------------------------------

/// `git grep -n executionMode` at v0.84.1 over `core/tools/` and `core/extensions/` hits only the
/// plumbing (`tool-definition-wrapper.ts:16`/`:44`, `extensions/types.ts:477`); NO built-in sets
/// it, and neither `edit.ts:303-311` nor `write.ts:192-198` declares one. Upstream serialization
/// for the mutators is `withFileMutationQueue` alone (edit.ts:316, write.ts:208), which cyrup
/// provides per-file via `FileMutationLocks`.
///
/// Declaring `Sequential` here made `cyrup-agent`'s `any_seq` (agent.rs:905-908) route the WHOLE
/// batch — reads and greps included — through `execute_sequential`. RED before; GREEN after.
#[test]
fn mutators_do_not_declare_sequential_execution() {
    let cwd = std::env::temp_dir();
    assert_eq!(write_tool(cwd.clone()).execution_mode(), ExecMode::Parallel);
    assert_eq!(edit_tool(cwd).execution_mode(), ExecMode::Parallel);
}

/// `edit.ts:310` `renderShell: "self"` — the only built-in that sets it. Every other built-in
/// leaves it unset, which is `"default"`.
#[test]
fn edit_declares_self_rendering_and_the_others_do_not() {
    let cwd = std::env::temp_dir();
    assert_eq!(
        edit_tool(cwd.clone()).render_kind(),
        ToolRenderKind::SelfRendered
    );
    assert_eq!(
        write_tool(cwd.clone()).render_kind(),
        ToolRenderKind::Default
    );
    assert_eq!(
        GrepTool::new(Arc::new(LocalFs), cwd.clone(), GrepOpts::default()).render_kind(),
        ToolRenderKind::Default
    );
    assert_eq!(
        FindTool::new(Arc::new(LocalFs), cwd.clone(), FindOpts::default()).render_kind(),
        ToolRenderKind::Default
    );
    assert_eq!(
        LsTool::new(Arc::new(LocalFs), cwd, LsOpts::default()).render_kind(),
        ToolRenderKind::Default
    );
}

// ---------------------------------------------------------------------------------------------
// TOOL-014 — `edit`'s access-failure body is pi's bare `Error code: <ERRNO>`.
// ---------------------------------------------------------------------------------------------

/// `edit.ts:332-334`:
/// ```ts
/// const errorMessage =
///   error instanceof Error && "code" in error ? `Error code: ${error.code}` : String(error);
/// throw new Error(`Could not edit file: ${path}. ${errorMessage}.`);
/// ```
/// — the BARE errno token, never the full Node message. cyrup interpolated the whole `ToolError`
/// (`"<abs path>: No such file or directory (os error 2)"`), so the machine-readable code the
/// model is trained on was absent. RED before; GREEN after.
#[cfg(unix)]
#[tokio::test]
async fn edit_access_failure_body_is_pis_bare_errno_token() {
    let dir = tempfile::tempdir().unwrap();
    let edit = edit_tool(dir.path().to_path_buf());
    let err = edit
        .execute(
            cid(),
            serde_json::json!({
                "path": "missing.txt",
                "edits": [{ "oldText": "a", "newText": "b" }]
            }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Could not edit file: missing.txt. Error code: ENOENT."
    );
}

/// The EACCES arm of the same ternary — a class the model acts on differently from ENOENT.
#[cfg(unix)]
#[tokio::test]
async fn edit_access_failure_reports_eacces_distinctly() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    let p = cwd.join("ro.txt");
    std::fs::write(&p, "hello\n").unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o400)).unwrap();
    // root bypasses the mode bits, exactly as it does for Node. Skip rather than assert.
    if std::fs::OpenOptions::new().write(true).open(&p).is_ok() {
        let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644));
        return;
    }

    let edit = edit_tool(cwd);
    let err = edit
        .execute(
            cid(),
            serde_json::json!({ "path": "ro.txt", "edits": [{ "oldText": "hello", "newText": "x" }] }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap_err();
    let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644));
    assert_eq!(
        err.to_string(),
        "Could not edit file: ro.txt. Error code: EACCES."
    );
}

// ---------------------------------------------------------------------------------------------
// TOOL-029 — `ls`'s readdir failure carries pi's third stable prefix.
// ---------------------------------------------------------------------------------------------

/// `ls.ts:147-152` `catch (e: any) { reject(new Error(`Cannot read directory: ${e.message}`)) }` —
/// a third stable prefix beside `Path not found:` (ls.ts:129) and `Not a directory:` (ls.ts:141),
/// distinguishing "exists, is a directory, cannot be enumerated". cyrup propagated `FsOps`'
/// raw `"<path>: <io error>"` wrapper, which carries none of the three. RED before; GREEN after.
#[cfg(unix)]
#[tokio::test]
async fn ls_readdir_failure_carries_pis_cannot_read_directory_prefix() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().join("locked");
    std::fs::create_dir(&d).unwrap();
    // 0300: traversal allowed (so `metadata` still says "is a directory"), readdir denied.
    std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o300)).unwrap();
    if std::fs::read_dir(&d).is_ok() {
        let _ = std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755));
        return; // running as root
    }

    let ls = LsTool::new(
        Arc::new(LocalFs),
        dir.path().to_path_buf(),
        LsOpts::default(),
    );
    let err = ls
        .execute(
            cid(),
            serde_json::json!({ "path": "locked" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap_err();
    let _ = std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755));
    let msg = err.to_string();
    assert!(msg.starts_with("Cannot read directory: "), "got: {msg}");
    // The body is `e.message`, which on the Node side leads with the errno code.
    assert!(
        msg.contains("EACCES"),
        "body must be Node-shaped, got: {msg}"
    );
    assert!(!msg.starts_with("Path not found:"), "got: {msg}");
    assert!(!msg.starts_with("Not a directory:"), "got: {msg}");
}

// ---------------------------------------------------------------------------------------------
// TOOL-041 — the post-write cancellation check.
// ---------------------------------------------------------------------------------------------

/// An `FsOps` whose `write_in_place` succeeds and cancels the token on the way out — the exact
/// interleaving pi's second `throwIfAborted()` exists to observe (write.ts:224 / edit.ts:352).
struct CancelOnWrite {
    inner: LocalFs,
    cancel: CancelToken,
}

#[async_trait::async_trait]
impl FsOps for CancelOnWrite {
    async fn read(&self, path: &Path) -> Result<Vec<u8>, ToolError> {
        self.inner.read(path).await
    }
    async fn write_in_place(&self, path: &Path, bytes: &[u8]) -> Result<(), ToolError> {
        let r = self.inner.write_in_place(path, bytes).await;
        self.cancel.cancel();
        r
    }
    async fn access(&self, path: &Path, mode: Access) -> Result<(), ToolError> {
        self.inner.access(path, mode).await
    }
    async fn metadata(&self, path: &Path) -> Result<Meta, ToolError> {
        self.inner.metadata(path).await
    }
    async fn read_dir(&self, path: &Path) -> Result<Vec<DirEntry>, ToolError> {
        self.inner.read_dir(path).await
    }
    fn walk(&self, root: &Path, opts: WalkOpts) -> EventStream<Result<WalkItem, ToolError>> {
        self.inner.walk(root, opts)
    }
}

/// `write.ts` brackets the write on BOTH sides — `throwIfAborted()` at `:220` before
/// `ops.writeFile` (`:223`) and again at `:224` immediately after, before the success value is
/// built (`throwIfAborted` defined at `:213-215`, throwing `"Operation aborted"`). Present at the
/// ported v0.83.0 too (`:219`), so this is baseline debt rather than drift.
///
/// cyrup checked only before the write, so a cancel landing during it produced
/// `Successfully wrote N bytes` where pi reports an aborted tool error. Pi does NOT undo the
/// write — the bytes stay on disk and only the RESULT is reported as aborted — which this pins on
/// both halves. RED before; GREEN after.
#[tokio::test]
async fn write_rechecks_cancellation_after_the_write_lands() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    let cancel = CancelToken::new();
    let fs = Arc::new(CancelOnWrite {
        inner: LocalFs,
        cancel: cancel.clone(),
    });
    let write = WriteTool::new(
        fs,
        Arc::new(FileMutationLocks::new()),
        cwd.clone(),
        WriteOpts,
    );

    let err = write
        .execute(
            cid(),
            serde_json::json!({ "path": "out.txt", "content": "payload\n" }),
            cancel,
            noop_sink(),
        )
        .await
        .expect_err("a cancel observed after the write must not report success");
    assert_eq!(err.to_string(), "Operation aborted");
    // Pi leaves the file exactly as the write left it.
    assert_eq!(
        std::fs::read_to_string(cwd.join("out.txt")).unwrap(),
        "payload\n"
    );
}

/// `edit.ts:352` is the sibling check — `throwIfAborted()` immediately after
/// `await ops.writeFile(absolutePath, finalContent)` (`:351`), before the diff is generated.
#[tokio::test]
async fn edit_rechecks_cancellation_after_the_write_lands() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    std::fs::write(cwd.join("f.txt"), "hello\n").unwrap();
    let cancel = CancelToken::new();
    let fs = Arc::new(CancelOnWrite {
        inner: LocalFs,
        cancel: cancel.clone(),
    });
    let edit = EditTool::new(
        fs,
        Arc::new(FileMutationLocks::new()),
        cwd.clone(),
        EditOpts,
    );

    let err = edit
        .execute(
            cid(),
            serde_json::json!({
                "path": "f.txt",
                "edits": [{ "oldText": "hello", "newText": "bye" }]
            }),
            cancel,
            noop_sink(),
        )
        .await
        .expect_err("a cancel observed after the write must not report success");
    assert_eq!(err.to_string(), "Operation aborted");
    assert_eq!(std::fs::read_to_string(cwd.join("f.txt")).unwrap(), "bye\n");
}

// ---------------------------------------------------------------------------------------------
// TOOL-018 — an `oldText` that NORMALIZES to empty takes pi's duplicate arm, not not-found.
// ---------------------------------------------------------------------------------------------

/// Both sides reject a LITERALLY empty `oldText` up front (`getEmptyOldTextError`,
/// edit-diff.ts:300-306, reached at `:315-319`), so this is only reachable when `oldText` is
/// non-empty but normalizes to empty — i.e. entirely whitespace, which `normalizeForFuzzyMatch`
/// strips.
///
/// There, `fuzzyContent.indexOf("")` is 0 (FOUND, edit-diff.ts:222) and
/// `fuzzyContent.split("").length - 1` is the UTF-16 code-unit count minus one (edit-diff.ts:255),
/// so pi raises the DUPLICATE-occurrences error at `:333`. cyrup's two empty-needle guards sent it
/// to `Could not find the exact text in …` instead, giving the model different remediation advice.
/// RED before; GREEN after.
#[tokio::test]
async fn whitespace_only_old_text_reports_duplicates_not_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    // No exact match for "   " so the fuzzy path runs; `normalizeForFuzzyMatch` strips the
    // trailing whitespace of every line, leaving the needle empty.
    std::fs::write(cwd.join("f.txt"), "alpha\nbeta\n").unwrap();
    let edit = edit_tool(cwd);

    let err = edit
        .execute(
            cid(),
            serde_json::json!({ "path": "f.txt", "edits": [{ "oldText": "   ", "newText": "x" }] }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap_err();
    let msg = err.to_string();
    // `getDuplicateError` with `totalEdits === 1` (edit-diff.ts:269-272).
    assert!(msg.starts_with("Found "), "got: {msg}");
    assert!(
        msg.ends_with(
            "occurrences of the text in f.txt. The text must be unique. \
             Please provide more context to make it unique."
        ),
        "got: {msg}"
    );
    assert!(!msg.contains("Could not find the exact text"), "got: {msg}");
}

/// The literally-empty case is unchanged and still takes pi's dedicated up-front error
/// (edit-diff.ts:302 `oldText must not be empty in ${path}.`).
#[tokio::test]
async fn literally_empty_old_text_still_takes_the_dedicated_error() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    std::fs::write(cwd.join("f.txt"), "alpha\n").unwrap();
    let edit = edit_tool(cwd);
    let err = edit
        .execute(
            cid(),
            serde_json::json!({ "path": "f.txt", "edits": [{ "oldText": "", "newText": "x" }] }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "oldText must not be empty in f.txt.");
}

// ---------------------------------------------------------------------------------------------
// TOOL-023 / TOOL-033 — the limit bounds the TRAVERSAL, not just the output vector.
// ---------------------------------------------------------------------------------------------

/// A walker that counts how many entries the tool actually pulled. `fd --max-results N`
/// (find.ts:252) makes fd itself stop traversing, and pi's grep line handler kills the rg child
/// the instant the cap fires (`stopChild(true)`, grep.ts:292-295 / `:240-245`) — so upstream never
/// finishes the walk. Draining the stream and truncating afterwards is invisible to output-only
/// assertions, which is why the probe counts pulls.
struct CountingWalk {
    inner: LocalFs,
    pulled: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait::async_trait]
impl FsOps for CountingWalk {
    async fn read(&self, path: &Path) -> Result<Vec<u8>, ToolError> {
        self.inner.read(path).await
    }
    async fn read_stream(&self, path: &Path) -> Result<Box<dyn std::io::Read + Send>, ToolError> {
        self.inner.read_stream(path).await
    }
    async fn write_in_place(&self, path: &Path, bytes: &[u8]) -> Result<(), ToolError> {
        self.inner.write_in_place(path, bytes).await
    }
    async fn access(&self, path: &Path, mode: Access) -> Result<(), ToolError> {
        self.inner.access(path, mode).await
    }
    async fn metadata(&self, path: &Path) -> Result<Meta, ToolError> {
        self.inner.metadata(path).await
    }
    async fn read_dir(&self, path: &Path) -> Result<Vec<DirEntry>, ToolError> {
        self.inner.read_dir(path).await
    }
    fn walk(&self, root: &Path, opts: WalkOpts) -> EventStream<Result<WalkItem, ToolError>> {
        use futures::StreamExt as _;
        let pulled = Arc::clone(&self.pulled);
        let inner = self.inner.walk(root, opts);
        Box::pin(inner.inspect(move |_| {
            pulled.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }))
    }
}

fn tree_of(n: usize) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..n {
        std::fs::write(dir.path().join(format!("f{i:04}.txt")), "NEEDLE\n").unwrap();
    }
    dir
}

/// TOOL-023 — `find` must stop pulling from the walk once `limit` rows exist, the way
/// `--max-results` (find.ts:252) stops fd. Previously the loop broke only on `None` from the
/// walker (`find.rs`), so every call paid the full-tree walk regardless of `limit`.
/// RED before (`pulled` was the whole tree); GREEN after.
#[tokio::test]
async fn find_stops_walking_once_the_limit_is_reached() {
    let dir = tree_of(300);
    let pulled = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let fs = Arc::new(CountingWalk {
        inner: LocalFs,
        pulled: Arc::clone(&pulled),
    });
    let find = FindTool::new(fs, dir.path().to_path_buf(), FindOpts::default());

    let r = find
        .execute(
            cid(),
            serde_json::json!({ "pattern": "*.txt", "limit": 5 }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();

    let rows = first_text(&r);
    let rows = rows.split_once("\n\n[").map_or(rows.as_str(), |(a, _)| a);
    assert_eq!(rows.lines().count(), 5, "got: {rows}");
    let n = pulled.load(std::sync::atomic::Ordering::SeqCst);
    assert!(
        n < 300,
        "the walk must be bounded by the limit, not by the tree ({n} pulled)"
    );
}

/// TOOL-033 — the same for `grep`. Pi's handler sets `matchLimitReached` and calls
/// `stopChild(true)` at grep.ts:292-295 the moment `matchCount >= effectiveLimit`, so neither the
/// traversal nor any further file read happens. cyrup drained the whole walk, SORTED it, and only
/// then searched — so on a large repo the 100-match window came from the alphabetically-first
/// files (a systematic `a*`-biased sample) after a full-tree walk on every call.
/// RED before; GREEN after.
#[tokio::test]
async fn grep_stops_walking_once_the_match_limit_is_reached() {
    let dir = tree_of(300);
    let pulled = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let fs = Arc::new(CountingWalk {
        inner: LocalFs,
        pulled: Arc::clone(&pulled),
    });
    let grep = GrepTool::new(fs, dir.path().to_path_buf(), GrepOpts::default());

    let r = grep
        .execute(
            cid(),
            serde_json::json!({ "pattern": "NEEDLE", "limit": 5 }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();

    let text = first_text(&r);
    let rows = text.split_once("\n\n[").map_or(text.as_str(), |(a, _)| a);
    assert_eq!(rows.lines().count(), 5, "got: {text}");
    // The `N matches limit reached` notice (grep.ts:345-350) must still fire under the new
    // strategy — that is the half of the behaviour the restructure must not lose.
    assert!(
        text.contains("5 matches limit reached. Use limit=10 for more"),
        "got: {text}"
    );
    let n = pulled.load(std::sync::atomic::Ordering::SeqCst);
    assert!(
        n < 300,
        "the walk must be bounded by the limit, not by the tree ({n} pulled)"
    );
}

// ---------------------------------------------------------------------------------------------
// TOOL-011 — `find` path-globs match the ABSOLUTE candidate path.
// ---------------------------------------------------------------------------------------------

/// fd `--full-path` "matches against the absolute candidate path" — pi's own in-source note at
/// find.ts:254-256 — and find.ts:267 `args.push("--", effectivePattern, searchPath)` hands fd the
/// ABSOLUTE search path as its root, so every candidate fd tests is absolute. pi relativizes only
/// for OUTPUT (find.ts:321-326).
///
/// cyrup matched the search-root-RELATIVE path, so a pattern naming a directory ABOVE the search
/// root selected nothing where fd matches. RED before; GREEN after.
#[tokio::test]
async fn find_path_globs_match_the_absolute_candidate_path() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/a.ts"), "x").unwrap();

    // Search root is `<tmp>/repo/src`; the pattern names `repo/src`, an ancestor of the candidate
    // but ABOVE the search root. fd tests `<tmp>/repo/src/a.ts` against `**/repo/src/*.ts` and
    // matches; the relative candidate was just `a.ts`, which never could.
    let find = FindTool::new(Arc::new(LocalFs), repo.clone(), FindOpts::default());
    let r = find
        .execute(
            cid(),
            serde_json::json!({ "pattern": "repo/src/*.ts", "path": "src" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    assert_eq!(first_text(&r), "a.ts", "got: {}", first_text(&r));

    // The common case is unchanged: both sides prepend `**/`, so a path-containing pattern
    // relative to the search root still matches, and output stays relativized (find.ts:321-326).
    let find = FindTool::new(Arc::new(LocalFs), repo, FindOpts::default());
    let r = find
        .execute(
            cid(),
            serde_json::json!({ "pattern": "src/*.ts" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    assert_eq!(first_text(&r), "src/a.ts", "got: {}", first_text(&r));
}

// ---------------------------------------------------------------------------------------------
// TOOL-034 — `grep` searches through a streaming reader, not a whole-file `Vec<u8>`.
// ---------------------------------------------------------------------------------------------

/// An `FsOps` that refuses the whole-file `read` on the SEARCH path and serves only the streaming
/// handle, so a regression back to `FsOps::read` + `search_slice` is observable rather than merely
/// slower. Pi's search never touches the agent heap at all — it runs in a separate ripgrep child
/// (`spawn(rgPath, …)`, grep.ts:226); pi's own in-process read (`getFileLines`, grep.ts:206-218)
/// runs ONLY for files that actually matched and only on the `contextValue > 0` / non-UTF-8 path
/// (grep.ts:332-333), which is why `read` stays available here for exactly that.
struct StreamOnlySearch {
    inner: LocalFs,
    whole_file_reads: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait::async_trait]
impl FsOps for StreamOnlySearch {
    async fn read(&self, path: &Path) -> Result<Vec<u8>, ToolError> {
        self.whole_file_reads
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.read(path).await
    }
    async fn read_stream(&self, path: &Path) -> Result<Box<dyn std::io::Read + Send>, ToolError> {
        self.inner.read_stream(path).await
    }
    async fn write_in_place(&self, path: &Path, bytes: &[u8]) -> Result<(), ToolError> {
        self.inner.write_in_place(path, bytes).await
    }
    async fn access(&self, path: &Path, mode: Access) -> Result<(), ToolError> {
        self.inner.access(path, mode).await
    }
    async fn metadata(&self, path: &Path) -> Result<Meta, ToolError> {
        self.inner.metadata(path).await
    }
    async fn read_dir(&self, path: &Path) -> Result<Vec<DirEntry>, ToolError> {
        self.inner.read_dir(path).await
    }
    fn walk(&self, root: &Path, opts: WalkOpts) -> EventStream<Result<WalkItem, ToolError>> {
        self.inner.walk(root, opts)
    }
}

/// At `context == 0` pi formats straight from ripgrep's own captured `data.lines.text` and never
/// calls `getFileLines` (grep.ts:323-331), so the agent process reads the file ZERO times — the
/// search happened in the rg child. cyrup must therefore make zero `FsOps::read` calls on that
/// path: every candidate goes through `read_stream`. Previously `search_slice` allocated a full
/// `Vec<u8>` per candidate BEFORE binary detection could reject it, so a multi-GB artifact in the
/// tree was an RSS spike or an OOM kill of the session even when it matched nothing.
/// RED before (three whole-file reads: one per candidate); GREEN after (zero).
#[tokio::test]
async fn grep_search_path_never_materializes_a_candidate() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    std::fs::write(cwd.join("a.txt"), "one\nNEEDLE\nthree\n").unwrap();
    std::fs::write(cwd.join("b.txt"), "no hits here\n").unwrap();
    std::fs::write(cwd.join("c.txt"), "also nothing\n").unwrap();

    let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let fs = Arc::new(StreamOnlySearch {
        inner: LocalFs,
        whole_file_reads: Arc::clone(&reads),
    });
    let grep = GrepTool::new(fs, cwd, GrepOpts::default());

    let r = grep
        .execute(
            cid(),
            serde_json::json!({ "pattern": "NEEDLE" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    assert_eq!(first_text(&r), "a.txt:2: NEEDLE");
    assert_eq!(
        reads.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "context==0 re-reads nothing (grep.ts:323-331); the search itself must stream"
    );
}

/// The other half of pi's condition: with `context > 0` the block path runs `getFileLines`
/// (grep.ts:206-218) — a genuine whole-file read — but ONLY for a file that actually matched.
/// Two non-matching siblings must contribute no reads at all.
#[tokio::test]
async fn grep_context_path_reads_only_files_that_matched() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    std::fs::write(cwd.join("a.txt"), "one\nNEEDLE\nthree\n").unwrap();
    std::fs::write(cwd.join("b.txt"), "no hits here\n").unwrap();
    std::fs::write(cwd.join("c.txt"), "also nothing\n").unwrap();

    let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let fs = Arc::new(StreamOnlySearch {
        inner: LocalFs,
        whole_file_reads: Arc::clone(&reads),
    });
    let grep = GrepTool::new(fs, cwd, GrepOpts::default());

    let r = grep
        .execute(
            cid(),
            serde_json::json!({ "pattern": "NEEDLE", "context": 1 }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    assert_eq!(
        first_text(&r),
        "a.txt-1- one\na.txt:2: NEEDLE\na.txt-3- three"
    );
    assert_eq!(
        reads.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "exactly the one matching file, once (pi's `fileCache`, grep.ts:205/:215)"
    );
}

// ------------------------------------------------- TOOL-044: the serialized `details.truncation` --

/// **TOOL-044.** `details.truncation` is persisted on `ToolResultMessage` (pi
/// `packages/ai/src/types.ts:415-420`), so its serialized shape is interop surface. Two divergences
/// from pi's `TruncationResult` (`core/tools/truncate.ts:15-38` @v0.83.0) are pinned here.
///
/// 1. **`truncatedBy` is `"lines" | "bytes" | null` — always present** (`truncate.ts:21`). cyrup
///    carried `skip_serializing_if = "Option::is_none"`, so on an untruncated result the key was
///    absent, not null. RED before: `serde_json::to_value(..)` had no `truncatedBy` member at all.
/// 2. **`maxLines` on the byte-only path is `Number.MAX_SAFE_INTEGER`**, passed literally at all
///    four of pi's byte-only call sites (`grep.ts:335`, `find.ts:189`, `find.ts:324`,
///    `ls.ts:182`) and copied verbatim into the record. cyrup passed `usize::MAX`, writing
///    `18446744073709551615` where pi writes `9007199254740991`. RED before on the equality.
///
/// The `content` field pi also carries (`truncate.ts:17`) is the item's stated residual and is
/// deliberately NOT asserted here — see the doc comment on `crate::truncate::Truncation`.
#[test]
fn truncation_details_serialize_in_pis_shape() {
    use crate::truncate::{MAX_SAFE_INTEGER, TruncOpts, truncate_head};

    // (1) An untruncated result must still carry an explicit `truncatedBy: null`.
    let short = truncate_head("one\ntwo\n", TruncOpts::new(2000, 50 * 1024));
    assert!(
        !short.info.truncated,
        "fixture must be untruncated for the null case to be reachable"
    );
    let v = serde_json::to_value(&short.info).unwrap();
    let obj = v.as_object().unwrap();
    assert!(
        obj.contains_key("truncatedBy"),
        "pi's TruncationResult always carries truncatedBy (truncate.ts:21); cyrup dropped the key \
         when it was None. Got: {v}"
    );
    assert!(
        obj["truncatedBy"].is_null(),
        "an untruncated result reports truncatedBy: null, not a value. Got: {v}"
    );

    // (2) The byte-only sentinel is JS's MAX_SAFE_INTEGER, not usize::MAX.
    assert_eq!(
        MAX_SAFE_INTEGER, 9_007_199_254_740_991,
        "Number.MAX_SAFE_INTEGER is 2^53 - 1"
    );
    let bytes_only = truncate_head("a\nb\nc\n", TruncOpts::bytes_only(50 * 1024));
    assert_eq!(
        bytes_only.info.max_lines, MAX_SAFE_INTEGER,
        "byte-only callers (grep/find/ls) must record pi's maxLines sentinel"
    );
    let v = serde_json::to_value(&bytes_only.info).unwrap();
    assert_eq!(
        v["maxLines"],
        serde_json::json!(9_007_199_254_740_991u64),
        "the serialized record is what reaches the session file. Got: {v}"
    );

    // And the truncated case still reports a real value, so (1) did not just blanket-null it.
    let long = truncate_head(&"x\n".repeat(10), TruncOpts::new(3, 50 * 1024));
    assert!(long.info.truncated);
    let v = serde_json::to_value(&long.info).unwrap();
    assert_eq!(v["truncatedBy"], serde_json::json!("lines"));
}

// ---------------------------------------------------------------------------------------------
// TOOL-043 — bash's `promptGuidelines` diverges from the ported tag TWICE, and both deltas must
// carry a `[CYRUP-DELTA]` tag.
// ---------------------------------------------------------------------------------------------

/// The two divergences, each re-derived at its tag this pass rather than carried from the ledger:
///
/// * **wording** — cyrup emits `"You can inspect …"`, which is v0.84.1 `bash.ts:47` verbatim. The
///   ported baseline is v0.83.0, where `bash.ts:330` reads the bare imperative
///   `"Inspect PI_* environment variables for current model and session details."`. cyrup is
///   AHEAD of the ported tag on a model-facing prompt string.
/// * **variable family** — upstream says `PI_*` at both tags; cyrup says `CYRUP_*`, because
///   `config::session_env_scrub_keys` deletes the five `PI_*` session names from the child.
///
/// Neither is being reverted. What was missing is that neither was FINDABLE: the rationale was
/// present in prose, but `[CYRUP-DELTA` is the grep this project's parity sweeps run to enumerate
/// accepted divergences, and a divergence that only a full re-derivation can rediscover gets
/// re-derived — which is how this item was filed in the first place.
///
/// This is a source scan for the same reason `no_inherited_harness_stdio.rs` is: the artifact
/// under test is source text (a doc annotation), so no runtime observation can see it. RED before
/// the fix — `grep -rn '\[CYRUP-DELTA' crates/cyrup-tools/src/tools/bash.rs` returned exactly one
/// hit, the `AI_AGENT` one in `execute`, and zero in the `prompt_guidelines` block.
#[test]
fn bash_prompt_guideline_deltas_are_tagged_cyrup_delta() {
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/tools/bash.rs"),
    )
    .expect("crates/cyrup-tools/src/tools/bash.rs is readable");

    // Presence before absence: anchor on the real declaration, so a renamed/moved method fails
    // loudly here instead of making every assertion below vacuous.
    let guidelines_at = src
        .find("fn prompt_guidelines(")
        .expect("`ShellTool::prompt_guidelines` still exists");
    let snippet_at = src
        .find("fn prompt_snippet(")
        .expect("`ShellTool::prompt_snippet` still exists");
    assert!(
        snippet_at < guidelines_at,
        "the doc block scanned below is the one BETWEEN prompt_snippet and prompt_guidelines"
    );
    let doc = &src[snippet_at..guidelines_at];

    // The string itself is unchanged, and it is still gated on `expose_session_environment`
    // (v0.84.1 bash.ts:334) — pinned byte-for-byte in `pi_schema.rs`. Re-asserted here only so a
    // future edit cannot satisfy this test by deleting the guideline instead of tagging it.
    assert!(
        doc.contains("CYRUP_*") || src.contains("You can inspect CYRUP_* environment variables"),
        "the guideline under audit must still be the CYRUP_* one"
    );

    let tags: Vec<&str> = doc
        .match_indices("[CYRUP-DELTA")
        .map(|(i, _)| &doc[i..])
        .collect();
    assert_eq!(
        tags.len(),
        2,
        "TOOL-043: the guideline carries TWO independent divergences (v0.84.0 wording; PI_* -> \
         CYRUP_*) and each needs its own `[CYRUP-DELTA]` tag. Found {} in the doc block.",
        tags.len()
    );

    assert!(
        doc.contains("[CYRUP-DELTA — version lag, AHEAD of the ported tag; wording only]"),
        "the wording delta must be tagged as a version-lag-AHEAD, so a later v0.84.x uplift reads \
         it as already-ported-early rather than as already-done-at-tag"
    );
    assert!(
        doc.contains(
            "[CYRUP-DELTA — deliberate, value only; the variable-family name inside the \
                      string]"
        ),
        "the PI_* -> CYRUP_* rename must be tagged as a deliberate value-only delta"
    );
    // Each tag must name the upstream symbol it diverges from (the `[CYRUP-DELTA]` contract).
    assert!(
        doc.contains("bash.ts:47"),
        "the wording delta must cite v0.84.1 bash.ts:47"
    );
    assert!(
        doc.contains("bash.ts:330"),
        "the wording delta must cite v0.83.0 bash.ts:330"
    );
}

/// TOOL-044 limb 3 — `details.truncation.content`.
///
/// pi's `TruncationResult` declares `content` first (`truncate.ts:17` @v0.83.0) and every call
/// site stores the object WHOLE, so the truncated text is in `details` a second time:
/// `read.ts:294`/`:305`, `grep.ts:348`, `find.ts:199`/`:336`, `ls.ts:193`, and for `bash` both the
/// streaming `details` (`bash.ts:356`) and the final one (`:409`) — the latter two via
/// `snapshot.truncation = {...tailTruncation, …}` (`output-accumulator.ts:100-107`), where the
/// spread is what carries `content` across.
///
/// RED before the fix: `serde_json::to_value(&info)` had no `content` member at all on either
/// branch. Asserted over the SERIALIZED value rather than the struct because the serialized form
/// is what reaches the session file, which is the interop surface this whole item is about.
///
/// Both branches are covered on purpose. Only asserting the truncated one would be satisfiable by
/// writing `content` only when `truncated` is set — which pi does not do: pi's no-truncation
/// branch (`truncate.ts:87-101`) returns the input content verbatim inside the same object.
#[test]
fn truncation_details_carry_pis_content_field() {
    use crate::truncate::{TruncOpts, truncate_head, truncate_tail};

    // (1) Untruncated: pi returns the INPUT verbatim, trailing newline included.
    let short = truncate_head("one\ntwo\n", TruncOpts::new(2000, 50 * 1024));
    assert!(
        !short.info.truncated,
        "fixture must be untruncated for this branch to be reachable"
    );
    let v = serde_json::to_value(&short.info).unwrap();
    assert_eq!(
        v["content"],
        serde_json::json!("one\ntwo\n"),
        "an untruncated TruncationResult still carries `content`, verbatim. Got: {v}"
    );

    // (2) Head-truncated: `content` is the kept prefix, and it agrees with `Truncated::content`
    // — cyrup splits pi's one struct into two, so the two copies must never disagree.
    let long = truncate_head(&"x\n".repeat(10), TruncOpts::new(3, 50 * 1024));
    assert!(long.info.truncated);
    let v = serde_json::to_value(&long.info).unwrap();
    assert_eq!(v["content"], serde_json::json!(long.content.clone()));
    assert_eq!(v["content"], serde_json::json!("x\nx\nx"));

    // (3) Tail-truncated (`bash`'s path) — same invariant from the other end.
    let tail = truncate_tail(&"y\n".repeat(10), TruncOpts::new(2, 50 * 1024));
    assert!(tail.info.truncated);
    let v = serde_json::to_value(&tail.info).unwrap();
    assert_eq!(v["content"], serde_json::json!(tail.content.clone()));
    assert_eq!(v["content"], serde_json::json!("y\ny"));

    // (4) The first-line-exceeds-limit branch reports an EMPTY content, not an absent key
    // (`truncate.ts:103-119` returns `content: ""`).
    let wide = truncate_head("aaaaaaaaaa\nb\n", TruncOpts::new(2000, 4));
    assert!(wide.info.first_line_exceeds_limit);
    let v = serde_json::to_value(&wide.info).unwrap();
    assert_eq!(v["content"], serde_json::json!(""));

    // (5) An older cyrup session record has no `content` key; loading it must not hard-fail.
    let mut legacy = serde_json::to_value(&short.info).unwrap();
    legacy.as_object_mut().unwrap().remove("content");
    let back: crate::truncate::Truncation = serde_json::from_value(legacy).unwrap();
    assert_eq!(
        back.content, "",
        "the read side defaults; the write side never omits it"
    );
}
