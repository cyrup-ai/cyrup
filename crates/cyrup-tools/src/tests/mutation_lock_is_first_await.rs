//! `write::execute` and `edit::execute` must take the per-path mutation lock as their FIRST
//! `.await` (`write.rs:108`, `edit.rs:273`).
//!
//! Ordering is only preserved if the callers REACH `FileMutationLocks::guard` in dispatch order.
//! `cyrup-agent`'s `execute_parallel` hands each body on once it has been driven to its first
//! suspension point (`exec.rs:177-181`), so an `.await` inserted ABOVE `guard()` moves the handoff
//! to that earlier point and same-path mutations are once again granted in whatever order the
//! blocking pool finishes them. Nothing else in the workspace would notice: both writes succeed,
//! both tool calls report success, and the file simply holds the wrong payload.
//!
//! Asserted at runtime rather than by reading the source, because both files are edited by other
//! work. With the runtime's ONE blocking thread occupied, `tokio::fs::canonicalize` inside
//! `FileMutationLocks::key` provably cannot complete, so the first poll of `execute` is pinned
//! inside the lock — or it is not, and this file says so.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::config::{EditOpts, WriteOpts};
use crate::lock::{FileMutationLocks, registration_is_held};
use crate::ops::local::LocalFs;
use crate::ops::{Access, DirEntry, FsOps, Meta, WalkItem, WalkOpts};
use crate::tools::{EditTool, WriteTool};
use cyrup_core::{
    CancelToken, EventStream, Tool, ToolCallId, ToolError, ToolUpdate, ToolUpdateSink,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Poll `f` exactly once with a no-op waker.
fn poll_once<F: std::future::Future>(f: std::pin::Pin<&mut F>) -> std::task::Poll<F::Output> {
    f.poll(&mut std::task::Context::from_waker(std::task::Waker::noop()))
}

/// `LocalFs` that counts every seam call. Zero is the assertion.
struct CountingFs {
    inner: Arc<dyn FsOps>,
    calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl FsOps for CountingFs {
    async fn read(&self, path: &Path) -> Result<Vec<u8>, ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.read(path).await
    }
    async fn write_in_place(&self, path: &Path, bytes: &[u8]) -> Result<(), ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.write_in_place(path, bytes).await
    }
    async fn access(&self, path: &Path, mode: Access) -> Result<(), ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.access(path, mode).await
    }
    async fn metadata(&self, path: &Path) -> Result<Meta, ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.metadata(path).await
    }
    async fn read_dir(&self, path: &Path) -> Result<Vec<DirEntry>, ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.read_dir(path).await
    }
    fn walk(&self, root: &Path, opts: WalkOpts) -> EventStream<Result<WalkItem, ToolError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.walk(root, opts)
    }
}

/// One blocking thread, occupied; poll `tool.execute` once; assert it parked inside the lock.
fn assert_first_await_is_the_mutation_lock(
    build: impl FnOnce(
        Arc<dyn FsOps>,
        Arc<FileMutationLocks>,
        PathBuf,
    ) -> (Arc<dyn Tool>, serde_json::Value),
    seed: impl FnOnce(&Path),
) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async move {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path());

        let calls = Arc::new(AtomicUsize::new(0));
        let fs: Arc<dyn FsOps> = Arc::new(CountingFs {
            inner: Arc::new(LocalFs),
            calls: calls.clone(),
        });
        let (tool, args) = build(fs, Arc::new(FileMutationLocks::new()), dir.path().to_path_buf());

        let (release, hold) = std::sync::mpsc::channel::<()>();
        let hog = tokio::task::spawn_blocking(move || {
            let _ = hold.recv();
        });

        let sink: ToolUpdateSink = Box::new(|_u: ToolUpdate| {});
        let mut body = std::pin::pin!(tool.execute(
            ToolCallId::from("tc-first-await"),
            args,
            CancelToken::new(),
            sink,
        ));

        assert!(
            poll_once(body.as_mut()).is_pending(),
            "the only blocking thread is occupied, so the first poll must park inside \
             `FileMutationLocks::key`'s `canonicalize`"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "an `FsOps` call was made BEFORE the mutation lock was taken. `guard()` must be the \
             first `.await` of `execute` (write.rs:108 / edit.rs:273): `execute_parallel` hands the \
             batch on at the first suspension point (exec.rs:177-181), so anything awaited above \
             `guard()` moves the handoff and same-path mutations lose dispatch order"
        );
        assert!(
            registration_is_held(),
            "the first `.await` of `execute` is NOT `FileMutationLocks::guard` — some other await \
             was inserted above it. Same-path mutations are no longer granted in the order the \
             model issued them (DoD 1/2/3); nothing else in the suite observes this"
        );

        let _ = release.send(());
        hog.await.unwrap();
        let _ = body.await;
    });
}

#[test]
fn write_takes_the_mutation_lock_before_any_other_await() {
    assert_first_await_is_the_mutation_lock(
        |fs, locks, cwd| {
            let tool: Arc<dyn Tool> = Arc::new(WriteTool::new(fs, locks, cwd, WriteOpts));
            (tool, serde_json::json!({ "path": "f.txt", "content": "hello" }))
        },
        |_dir| {},
    );
}

#[test]
fn edit_takes_the_mutation_lock_before_any_other_await() {
    assert_first_await_is_the_mutation_lock(
        |fs, locks, cwd| {
            let tool: Arc<dyn Tool> = Arc::new(EditTool::new(fs, locks, cwd, EditOpts));
            (
                tool,
                serde_json::json!({
                    "path": "f.txt",
                    "edits": [{ "oldText": "SEED", "newText": "DONE" }],
                }),
            )
        },
        |dir| std::fs::write(dir.join("f.txt"), b"SEED\n").unwrap(),
    );
}
