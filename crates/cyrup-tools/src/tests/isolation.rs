//! Integration tests for the permissions/isolation seam pieces (arch-12, func-12 R-12-*).
//!
//! Covers the *testable* subset on this host:
//! - A-12-1: the default (no-gate) stance — `bash rm -rf` runs with no prompt/policy.
//! - A-12-3: `ProtectedFs` blocks `write`/`edit` to `.env`/`.git/`, passes reads through.
//! - traversal-root confinement + escape rejection (R-03-006).
//! - A-12-6/7/8 (as the operations seam, not real containers): swapping the backend re-targets all
//!   tools with no contract change, proven with a recording `FsOps`.
//! - A-12-2/5 (as policy units): `PermissionPolicy` Proceed/Mutate/Block/Confirm — see the unit
//!   tests in `src/isolation/policy.rs`.
//!
//! Deferred / out-of-crate (one sentence each):
//! - A-12-9 OS-sandbox (syscall/path restriction): deferred placeholder only — `landlock`/
//!   `seccompiler`/Seatbelt are not pulled and it is not testable on this host (`src/isolation/sandbox.rs`).
//! - A-12-6/7/8 *end-to-end* with real containers/micro-VMs/SSH: out of crate (needs docker/ssh); the
//!   seam is proven here, the remote backends are the same shape applied to a remote.
//! - A-12-4 confirm-destructive session lifecycle: lives in the agent hooks (arch-08).
//! - A-12-10 trust-gated extension/package loading: lives in `cyrup-config`/ext.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use crate::isolation::{ProtectedFs, ProtectedPaths, TraversalFs};
use crate::ops::local::LocalFs;
use crate::ops::{Access, Backend, DirEntry, FsOps, ImageMime, Meta, WalkItem, WalkOpts};
use crate::tools::{ReadTool, ShellTool, WriteTool};
use crate::{BashOpts, FileMutationLocks, ReadOpts, WriteOpts};
use cyrup_core::{
    CancelToken, Content, ToolCallId, ToolError, ToolResult, ToolUpdate, ToolUpdateSink,
};
use cyrup_core::{EventStream, Tool};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

fn cid() -> ToolCallId {
    ToolCallId::from("tc-iso")
}

fn noop_sink() -> ToolUpdateSink {
    Box::new(|_u: ToolUpdate| {})
}

fn first_text(r: &ToolResult) -> String {
    for c in &r.content {
        if let Content::Text { text, .. } = c {
            return text.to_string();
        }
    }
    String::new()
}

// ---------------------------------------------------------------- A-12-1 default no-gate stance

#[tokio::test]
async fn default_bash_rm_rf_runs_without_any_gate() {
    // R-12-001/002: with no policy/gate in the path, a destructive command runs directly.
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    let doomed = cwd.join("doomed");
    std::fs::create_dir(&doomed).unwrap();
    std::fs::write(doomed.join("f.txt"), "bye").unwrap();
    assert!(doomed.exists());

    let bash = ShellTool::bash(Backend::default().proc, cwd, BashOpts::default());
    let r = bash
        .execute(
            cid(),
            serde_json::json!({ "command": "rm -rf doomed" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    // Exit 0, no error, no prompt — the directory is gone.
    assert!(
        !doomed.exists(),
        "rm -rf should have run: {}",
        first_text(&r)
    );
}

// ---------------------------------------------------------------- A-12-3 protected paths

fn protected_fs() -> Arc<dyn FsOps> {
    Arc::new(ProtectedFs::new(
        Arc::new(LocalFs),
        ProtectedPaths::defaults(),
    ))
}

#[tokio::test]
async fn protected_paths_block_writes_pass_reads() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    let locks = Arc::new(FileMutationLocks::new());
    let write = WriteTool::new(protected_fs(), locks, cwd.clone(), WriteOpts);

    // Write to .env -> blocked.
    let err = write
        .execute(
            cid(),
            serde_json::json!({ "path": ".env", "content": "SECRET=1" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("protected"), "got: {err}");
    assert!(!cwd.join(".env").exists(), ".env must not be written");

    // Write inside .git/ -> blocked.
    let err = write
        .execute(
            cid(),
            serde_json::json!({ "path": ".git/config", "content": "x" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("protected"), "got: {err}");

    // Write to a normal file -> allowed.
    let ok = write
        .execute(
            cid(),
            serde_json::json!({ "path": "src/main.rs", "content": "fn main(){}" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    assert!(first_text(&ok).contains("Successfully wrote"));

    // Reads pass through even for protected paths: pre-create a .env on disk and read it.
    std::fs::write(cwd.join(".env"), "API=abc").unwrap();
    let read = ReadTool::new(protected_fs(), cwd, ReadOpts::default());
    let r = read
        .execute(
            cid(),
            serde_json::json!({ "path": ".env" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    assert!(
        first_text(&r).contains("API=abc"),
        "read should pass through: {}",
        first_text(&r)
    );
}

/// ADR-0003 D8(4), the in-crate half — **the guard's scope is the fs seam only**.
///
/// pi has no protected-path concept at all (`pi/packages/coding-agent/src/core/tools/write.ts:
/// 195-225` @v0.83.0 resolves the path and calls `ops.writeFile` with no predicate), so the
/// default backend must write `.env` like any other file; and even when an embedder opts in via
/// `SessionConfig::protect_paths`, only `fs` is decorated — `bash` reaches the same file, which is
/// why the flag is no longer on by default (ADR-0003 D5/D6).
///
/// Executable documentation: this is the assertion that makes the fs-only scope a contract rather
/// than a footnote.
#[tokio::test]
async fn protected_fs_is_fs_only_and_bash_is_never_covered() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();

    // (a) Undecorated (the shipped default after ADR-0003 D5): `write` to `.env` succeeds, exactly
    // like pi's `write.ts:195-225`.
    let plain: Arc<dyn FsOps> = Arc::new(LocalFs);
    let write_plain = WriteTool::new(
        plain,
        Arc::new(FileMutationLocks::new()),
        cwd.clone(),
        WriteOpts,
    );
    let ok = write_plain
        .execute(
            cid(),
            serde_json::json!({ "path": ".env", "content": "A=1\n" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    assert!(
        first_text(&ok).contains("Successfully wrote"),
        "got: {}",
        first_text(&ok)
    );
    assert_eq!(std::fs::read_to_string(cwd.join(".env")).unwrap(), "A=1\n");

    // (b) Embedder opt-in: `write` is refused …
    let write_guarded = WriteTool::new(
        protected_fs(),
        Arc::new(FileMutationLocks::new()),
        cwd.clone(),
        WriteOpts,
    );
    let err = write_guarded
        .execute(
            cid(),
            serde_json::json!({ "path": ".env", "content": "B=2\n" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("protected"), "got: {err}");

    // … and `bash` reaches the very same file anyway, because the PROCESS seam is undecorated.
    let bash = ShellTool::bash(Backend::default().proc, cwd.clone(), BashOpts::default());
    bash.execute(
        cid(),
        serde_json::json!({ "command": "printf 'C=3\\n' >> .env" }),
        CancelToken::new(),
        noop_sink(),
    )
    .await
    .unwrap();
    let after = std::fs::read_to_string(cwd.join(".env")).unwrap();
    assert!(
        after.contains("C=3"),
        "`bash` is not covered by ProtectedFs — that is the documented scope, and the reason the \
         flag defaults to false (ADR-0003 D5). Got: {after:?}"
    );
}

// ---------------------------------------------------------------- traversal root confinement

#[tokio::test]
async fn traversal_root_confines_and_rejects_escape() {
    let base = tempfile::tempdir().unwrap();
    let root = base.path().join("work");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(base.path().join("secret.txt"), "top secret").unwrap();
    std::fs::write(root.join("in.txt"), "inside").unwrap();

    let confined: Arc<dyn FsOps> = Arc::new(TraversalFs::new(Arc::new(LocalFs), root.clone()));

    // Read inside the root -> ok.
    let read = ReadTool::new(confined.clone(), root.clone(), ReadOpts::default());
    let r = read
        .execute(
            cid(),
            serde_json::json!({ "path": "in.txt" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    assert!(first_text(&r).contains("inside"));

    // Read escaping the root via ../ -> rejected (the `read` tool probes via `access`, whose
    // confinement denial it surfaces as not-found; either way the escape is blocked and the secret
    // is never returned).
    let err = read
        .execute(
            cid(),
            serde_json::json!({ "path": "../secret.txt" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap_err();
    assert!(
        !err.to_string().contains("top secret"),
        "secret must not leak: {err}"
    );

    // Write escaping the root -> rejected, no file created.
    let locks = Arc::new(FileMutationLocks::new());
    let write = WriteTool::new(confined, locks, root.clone(), WriteOpts);
    let err = write
        .execute(
            cid(),
            serde_json::json!({ "path": "../escape.txt", "content": "x" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("escapes"), "got: {err}");
    assert!(!base.path().join("escape.txt").exists());
}

// ---------------------------------------------------------------- A-12-6/7/8 backend-swap seam

/// A recording backend that proves tools are backend-agnostic: swapping it in re-targets every tool
/// with no contract change (R-12-011/012). The same shape applied to a remote is an SSH/container
/// backend.
struct RecordingFs {
    inner: Arc<dyn FsOps>,
    reads: Arc<AtomicUsize>,
    writes: Arc<AtomicUsize>,
    last_write: Arc<Mutex<Option<PathBuf>>>,
}

#[async_trait::async_trait]
impl FsOps for RecordingFs {
    async fn read(&self, path: &Path) -> Result<Vec<u8>, ToolError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.inner.read(path).await
    }
    async fn write_in_place(&self, path: &Path, bytes: &[u8]) -> Result<(), ToolError> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        *self.last_write.lock().unwrap() = Some(path.to_path_buf());
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
    fn detect_image_mime(&self, path: &Path) -> Option<ImageMime> {
        self.inner.detect_image_mime(path)
    }
    fn walk(&self, root: &Path, opts: WalkOpts) -> EventStream<Result<WalkItem, ToolError>> {
        self.inner.walk(root, opts)
    }
}

#[tokio::test]
async fn backend_swap_retargets_tools_without_contract_change() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    let reads = Arc::new(AtomicUsize::new(0));
    let writes = Arc::new(AtomicUsize::new(0));
    let last_write = Arc::new(Mutex::new(None));
    let recording: Arc<dyn FsOps> = Arc::new(RecordingFs {
        inner: Arc::new(LocalFs),
        reads: reads.clone(),
        writes: writes.clone(),
        last_write: last_write.clone(),
    });

    // Write through the swapped backend.
    let locks = Arc::new(FileMutationLocks::new());
    let write = WriteTool::new(recording.clone(), locks, cwd.clone(), WriteOpts);
    let w = write
        .execute(
            cid(),
            serde_json::json!({ "path": "out.txt", "content": "routed" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    // Identical tool contract/output regardless of backend.
    assert!(first_text(&w).contains("Successfully wrote 6 bytes to out.txt"));
    assert_eq!(
        writes.load(Ordering::SeqCst),
        1,
        "write routed through the swapped backend"
    );
    assert_eq!(
        last_write.lock().unwrap().as_ref().unwrap(),
        &cwd.join("out.txt")
    );

    // Read through the swapped backend sees the write-through content.
    let read = ReadTool::new(recording, cwd.clone(), ReadOpts::default());
    let r = read
        .execute(
            cid(),
            serde_json::json!({ "path": "out.txt" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    assert!(first_text(&r).contains("routed"));
    assert!(
        reads.load(Ordering::SeqCst) >= 1,
        "read routed through the swapped backend"
    );
    // The write actually landed on the host workspace (R-12-014 write-through analog).
    assert_eq!(
        std::fs::read_to_string(cwd.join("out.txt")).unwrap(),
        "routed"
    );
}

// -------------------------------------------------------- decorator delegation completeness

/// An `FsOps` whose `read_stream` returns something a whole-file `read` CANNOT produce.
///
/// The distinct value is the whole design of this probe. `FsOps::read_stream`'s default body is
/// `Cursor::new(self.read(path).await?)` (`ops/mod.rs:329-334`), so a decorator that forgets to
/// forward `read_stream` still yields byte-for-byte the same content as one that forwards it — a
/// dropped delegation and the trait default are observationally identical on content alone, which
/// is exactly why this omission survived TOOL-034's landing and both later sweeps. Making the two
/// paths return DIFFERENT bytes is what turns the assertion from vacuous into a real one.
struct DistinctStreamFs;

#[async_trait::async_trait]
impl FsOps for DistinctStreamFs {
    async fn read(&self, _path: &Path) -> Result<Vec<u8>, ToolError> {
        Ok(b"WHOLE-READ".to_vec())
    }
    async fn read_stream(&self, _path: &Path) -> Result<Box<dyn std::io::Read + Send>, ToolError> {
        Ok(Box::new(std::io::Cursor::new(b"REAL-STREAM".to_vec())))
    }
    async fn write_in_place(&self, _path: &Path, _bytes: &[u8]) -> Result<(), ToolError> {
        Ok(())
    }
    async fn access(&self, _path: &Path, _mode: Access) -> Result<(), ToolError> {
        Ok(())
    }
    async fn metadata(&self, _path: &Path) -> Result<Meta, ToolError> {
        Err(ToolError::new("metadata unused by this probe"))
    }
    async fn read_dir(&self, _path: &Path) -> Result<Vec<DirEntry>, ToolError> {
        Ok(Vec::new())
    }
    /// The SECOND defaulted method on `FsOps`, and the one this probe previously left unpinned.
    ///
    /// The trait default classifies by file EXTENSION
    /// (`path.extension() → ImageMime::from_extension`, `ops/mod.rs:363-365`). This override
    /// deliberately contradicts it in both directions, so neither answer can be produced by the
    /// default: a `.txt` path (default `None`) reports `Png`, and a `.png` path (default
    /// `Some(Png)`) reports `None`.
    fn detect_image_mime(&self, path: &Path) -> Option<ImageMime> {
        match path.extension().and_then(|e| e.to_str()) {
            Some("png") => None,
            _ => Some(ImageMime::Png),
        }
    }
    fn walk(&self, _root: &Path, _opts: WalkOpts) -> EventStream<Result<WalkItem, ToolError>> {
        Box::pin(tokio_stream::empty())
    }
}

async fn stream_text(fs: &dyn FsOps, path: &Path) -> String {
    use std::io::Read as _;
    let mut reader = fs.read_stream(path).await.expect("read_stream succeeds");
    let mut out = String::new();
    reader
        .read_to_string(&mut out)
        .expect("probe stream is utf-8");
    out
}

/// Every `FsOps` decorator must forward `read_stream` to its inner seam rather than inherit the
/// trait's whole-file default.
///
/// The defect this pins is the Rust half of a JS/Rust asymmetry: pi's operation decorators are
/// object literals (`{ ...ops, writeFile }`, e.g. `write.ts:32-35` / `edit.ts:83-87` style), so a
/// method added to the seam later is carried through BY CONSTRUCTION. A Rust decorator has to name
/// every method, and omitting one silently substitutes the trait default. Here that default is
/// `Cursor::new(self.read(..))`, so `confineToCwd` or `protectPaths` quietly reverted `grep` to the
/// whole-file materialization TOOL-034 removed — correct output, wrong memory profile, no failing
/// test anywhere.
#[tokio::test]
async fn fs_decorators_forward_read_stream_instead_of_inheriting_the_whole_file_default() {
    let base: Arc<dyn FsOps> = Arc::new(DistinctStreamFs);
    let root = std::env::temp_dir();
    let probe = root.join("decorator-delegation-probe.txt");

    // Presence before absence: the probe itself must distinguish the two paths, or every assertion
    // below is vacuous.
    assert_eq!(stream_text(&*base, &probe).await, "REAL-STREAM");
    assert_eq!(base.read(&probe).await.unwrap(), b"WHOLE-READ".to_vec());

    let traversal: Arc<dyn FsOps> = Arc::new(TraversalFs::new(base.clone(), root.clone()));
    assert_eq!(
        stream_text(&*traversal, &probe).await,
        "REAL-STREAM",
        "TraversalFs must forward read_stream; inheriting the default silently drops LocalFs's \
         real-File streaming and re-opens TOOL-034 whenever confineToCwd is on"
    );

    let protected: Arc<dyn FsOps> =
        Arc::new(ProtectedFs::new(base.clone(), ProtectedPaths::defaults()));
    assert_eq!(
        stream_text(&*protected, &probe).await,
        "REAL-STREAM",
        "ProtectedFs must forward read_stream for the same reason"
    );

    // Stacked exactly as `cyrup-session-svc/src/builder.rs:753-758` stacks them when both settings
    // are on — the configuration where the loss actually shipped.
    let stacked: Arc<dyn FsOps> = Arc::new(ProtectedFs::new(
        Arc::new(TraversalFs::new(base, root.clone())),
        ProtectedPaths::defaults(),
    ));
    assert_eq!(stream_text(&*stacked, &probe).await, "REAL-STREAM");

    // TraversalFs must still CONFINE on this method, not merely pass it through: a path outside the
    // root is rejected before the inner seam is opened.
    let confined = TraversalFs::new(
        Arc::new(DistinctStreamFs),
        root.join("cyrup-decorator-root"),
    );
    assert!(
        confined
            .read_stream(Path::new("/etc/passwd"))
            .await
            .is_err(),
        "read_stream must apply the traversal guard, not just delegate"
    );
}

/// The companion to the test above for `FsOps`' OTHER defaulted method.
///
/// `detect_image_mime` is the second place a decorator can silently substitute the trait default
/// (`ops/mod.rs:363-365`, extension-based classification). Both decorators DO forward it
/// (`protected.rs:145-147`, `traversal.rs:123-125`) — this test is what keeps that true, and it is
/// written so a deleted forward FAILS rather than agreeing with the default: the probe answers
/// `Some(Png)` where the default answers `None`, and `None` where the default answers `Some(Png)`.
#[test]
fn fs_decorators_forward_detect_image_mime_instead_of_inheriting_the_extension_default() {
    let base: Arc<dyn FsOps> = Arc::new(DistinctStreamFs);
    let root = std::env::temp_dir();
    let text = root.join("probe.txt");
    let png = root.join("probe.png");

    // Presence before absence: the probe must disagree with the default in BOTH directions, or the
    // assertions below can be satisfied by a decorator that forwards nothing.
    assert_eq!(base.detect_image_mime(&text), Some(ImageMime::Png));
    assert_eq!(base.detect_image_mime(&png), None);

    let traversal: Arc<dyn FsOps> = Arc::new(TraversalFs::new(base.clone(), root.clone()));
    let protected: Arc<dyn FsOps> =
        Arc::new(ProtectedFs::new(base.clone(), ProtectedPaths::defaults()));
    let stacked: Arc<dyn FsOps> = Arc::new(ProtectedFs::new(
        Arc::new(TraversalFs::new(base, root.clone())),
        ProtectedPaths::defaults(),
    ));

    for (label, fs) in [
        ("TraversalFs", &traversal),
        ("ProtectedFs", &protected),
        ("ProtectedFs∘TraversalFs", &stacked),
    ] {
        assert_eq!(
            fs.detect_image_mime(&text),
            Some(ImageMime::Png),
            "{label} must forward detect_image_mime; the extension default would answer None"
        );
        assert_eq!(
            fs.detect_image_mime(&png),
            None,
            "{label} must forward detect_image_mime; the extension default would answer Some(Png)"
        );
    }
}
