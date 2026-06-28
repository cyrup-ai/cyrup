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

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_core::{CancelToken, Content, ToolCallId, ToolError, ToolResult, ToolUpdate, ToolUpdateSink};
use cyrup_tools::isolation::{ProtectedFs, ProtectedPaths, TraversalFs};
use cyrup_tools::ops::local::LocalFs;
use cyrup_tools::ops::{Access, Backend, DirEntry, FsOps, ImageMime, Meta, ShellConfig, WalkItem, WalkOpts};
use cyrup_tools::tools::{BashTool, ReadTool, WriteTool};
use cyrup_tools::{BashOpts, FileMutationLocks, ReadOpts, WriteOpts};
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
            return text.clone();
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

    let bash = BashTool::new(Backend::default().proc, ShellConfig::detect(), cwd, BashOpts::default());
    let r = bash
        .execute(cid(), serde_json::json!({ "command": "rm -rf doomed" }), CancelToken::new(), noop_sink())
        .await
        .unwrap();
    // Exit 0, no error, no prompt — the directory is gone.
    assert!(!doomed.exists(), "rm -rf should have run: {}", first_text(&r));
}

// ---------------------------------------------------------------- A-12-3 protected paths

fn protected_fs() -> Arc<dyn FsOps> {
    Arc::new(ProtectedFs::new(Arc::new(LocalFs), ProtectedPaths::defaults()))
}

#[tokio::test]
async fn protected_paths_block_writes_pass_reads() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    let locks = Arc::new(FileMutationLocks::new());
    let write = WriteTool::new(protected_fs(), locks, cwd.clone(), WriteOpts);

    // Write to .env -> blocked.
    let err = write
        .execute(cid(), serde_json::json!({ "path": ".env", "content": "SECRET=1" }), CancelToken::new(), noop_sink())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("protected"), "got: {err}");
    assert!(!cwd.join(".env").exists(), ".env must not be written");

    // Write inside .git/ -> blocked.
    let err = write
        .execute(cid(), serde_json::json!({ "path": ".git/config", "content": "x" }), CancelToken::new(), noop_sink())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("protected"), "got: {err}");

    // Write to a normal file -> allowed.
    let ok = write
        .execute(cid(), serde_json::json!({ "path": "src/main.rs", "content": "fn main(){}" }), CancelToken::new(), noop_sink())
        .await
        .unwrap();
    assert!(first_text(&ok).contains("Wrote"));

    // Reads pass through even for protected paths: pre-create a .env on disk and read it.
    std::fs::write(cwd.join(".env"), "API=abc").unwrap();
    let read = ReadTool::new(protected_fs(), cwd, ReadOpts::default());
    let r = read
        .execute(cid(), serde_json::json!({ "path": ".env" }), CancelToken::new(), noop_sink())
        .await
        .unwrap();
    assert!(first_text(&r).contains("API=abc"), "read should pass through: {}", first_text(&r));
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
        .execute(cid(), serde_json::json!({ "path": "in.txt" }), CancelToken::new(), noop_sink())
        .await
        .unwrap();
    assert!(first_text(&r).contains("inside"));

    // Read escaping the root via ../ -> rejected (the `read` tool probes via `access`, whose
    // confinement denial it surfaces as not-found; either way the escape is blocked and the secret
    // is never returned).
    let err = read
        .execute(cid(), serde_json::json!({ "path": "../secret.txt" }), CancelToken::new(), noop_sink())
        .await
        .unwrap_err();
    assert!(!err.to_string().contains("top secret"), "secret must not leak: {err}");

    // Write escaping the root -> rejected, no file created.
    let locks = Arc::new(FileMutationLocks::new());
    let write = WriteTool::new(confined, locks, root.clone(), WriteOpts);
    let err = write
        .execute(cid(), serde_json::json!({ "path": "../escape.txt", "content": "x" }), CancelToken::new(), noop_sink())
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
    async fn write_atomic(&self, path: &Path, bytes: &[u8]) -> Result<(), ToolError> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        *self.last_write.lock().unwrap() = Some(path.to_path_buf());
        self.inner.write_atomic(path, bytes).await
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
        .execute(cid(), serde_json::json!({ "path": "out.txt", "content": "routed" }), CancelToken::new(), noop_sink())
        .await
        .unwrap();
    // Identical tool contract/output regardless of backend.
    assert!(first_text(&w).contains("Wrote 6 bytes to out.txt"));
    assert_eq!(writes.load(Ordering::SeqCst), 1, "write routed through the swapped backend");
    assert_eq!(last_write.lock().unwrap().as_ref().unwrap(), &cwd.join("out.txt"));

    // Read through the swapped backend sees the write-through content.
    let read = ReadTool::new(recording, cwd.clone(), ReadOpts::default());
    let r = read
        .execute(cid(), serde_json::json!({ "path": "out.txt" }), CancelToken::new(), noop_sink())
        .await
        .unwrap();
    assert!(first_text(&r).contains("routed"));
    assert!(reads.load(Ordering::SeqCst) >= 1, "read routed through the swapped backend");
    // The write actually landed on the host workspace (R-12-014 write-through analog).
    assert_eq!(std::fs::read_to_string(cwd.join("out.txt")).unwrap(), "routed");
}
