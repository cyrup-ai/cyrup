//! `find`'s abort must be observed BEFORE any filesystem work and on EVERY data path — TOOL-041.
//!
//! Pi `coding-agent/src/core/tools/find.ts`:
//!   * `:142-145` — the first statement inside the promise executor is
//!     `if (signal?.aborted) { reject(new Error("Operation aborted")); return; }`, ahead of
//!     `resolveToCwd`, `ops.exists`, the fd download, everything;
//!   * `:158-160` — an `abort` listener registered `{ once: true }` rejects the instant the signal
//!     fires, and calls `stopChild()` so the child stops producing;
//!   * `:174`, `:182`, `:226`, `:299`, `:355` — every data path re-tests `signal?.aborted` FIRST,
//!     before processing what it just received.
//!
//! Data can therefore never win a race against an already-fired abort upstream. cyrup observed the
//! token in exactly ONE place — an UNBIASED `tokio::select!` inside the walk loop — which cost it
//! both edges:
//!
//!   * an already-cancelled `find` still ran `fs.metadata(search_root)` and the whole
//!     `inside_git_repo` ancestor walk (one `metadata` per parent, up to the filesystem root)
//!     before it could report the abort;
//!   * `select!` without `biased;` polls its arms in RANDOM order, so with the token cancelled and
//!     a directory entry already buffered the walk arm won ~half the time and the tool kept
//!     consuming entries after Esc.
//!
//! The sibling `grep.rs` already carried the loop-top guard (grep.rs, `if cancel.is_cancelled()`);
//! `find.rs` had neither that nor `biased;`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use crate::config::FindOpts;
use crate::ops::local::LocalFs;
use crate::ops::{Access, DirEntry, FsOps, Meta, WalkItem, WalkOpts};
use crate::tools::FindTool;
use cyrup_core::{CancelToken, EventStream, Tool, ToolCallId, ToolError, ToolUpdate, ToolUpdateSink};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn cid() -> ToolCallId {
    ToolCallId::from("tc-find-abort")
}

fn noop_sink() -> ToolUpdateSink {
    Box::new(|_u: ToolUpdate| {})
}

fn tree_of(n: usize) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..n {
        std::fs::write(dir.path().join(format!("f{i:04}.txt")), "x\n").unwrap();
    }
    dir
}

/// Counts every `metadata` call and every walk entry actually pulled, and optionally cancels the
/// token as the Nth entry leaves the stream — the closest in-process stand-in for "the user pressed
/// Esc while the walk was mid-flight".
struct AbortProbeFs {
    inner: LocalFs,
    metadata_calls: Arc<AtomicUsize>,
    pulled: Arc<AtomicUsize>,
    /// Cancel `token` once this many entries have been pulled. `0` disables.
    cancel_at: usize,
    token: CancelToken,
}

#[async_trait::async_trait]
impl FsOps for AbortProbeFs {
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
        self.metadata_calls.fetch_add(1, Ordering::SeqCst);
        self.inner.metadata(path).await
    }
    async fn read_dir(&self, path: &Path) -> Result<Vec<DirEntry>, ToolError> {
        self.inner.read_dir(path).await
    }
    fn walk(&self, root: &Path, opts: WalkOpts) -> EventStream<Result<WalkItem, ToolError>> {
        use futures::StreamExt as _;
        let pulled = Arc::clone(&self.pulled);
        let token = self.token.clone();
        let cancel_at = self.cancel_at;
        let inner = self.inner.walk(root, opts);
        Box::pin(inner.inspect(move |_| {
            let n = pulled.fetch_add(1, Ordering::SeqCst) + 1;
            if cancel_at != 0 && n == cancel_at {
                token.cancel();
            }
        }))
    }
}

/// An ALREADY-cancelled `find` must do no filesystem work at all — pi's find.ts:142-145.
///
/// RED before: `execute` reached the walk loop's `select!` before it ever looked at the token, so
/// `metadata_calls` was non-zero (the `search_root` stat plus one `.git` probe per ancestor of the
/// temp dir, which on macOS is `/var/folders/**` — four or more calls). GREEN after: zero.
#[tokio::test]
async fn a_precancelled_find_touches_the_filesystem_zero_times() {
    let dir = tree_of(8);
    let cancel = CancelToken::new();
    cancel.cancel();

    let metadata_calls = Arc::new(AtomicUsize::new(0));
    let pulled = Arc::new(AtomicUsize::new(0));
    let fs = Arc::new(AbortProbeFs {
        inner: LocalFs,
        metadata_calls: Arc::clone(&metadata_calls),
        pulled: Arc::clone(&pulled),
        cancel_at: 0,
        token: cancel.clone(),
    });

    let find = FindTool::new(fs, dir.path().to_path_buf(), FindOpts::default());
    let err = find
        .execute(cid(), serde_json::json!({ "pattern": "*.txt" }), cancel, noop_sink())
        .await
        .expect_err("an already-aborted find must reject, not search");

    // The literal is pi's, verbatim (find.ts:143) — the model sees this string.
    assert_eq!(err.to_string(), "Operation aborted");
    assert_eq!(
        metadata_calls.load(Ordering::SeqCst),
        0,
        "pi checks `signal?.aborted` before `ops.exists` and before the git-repo probe \
         (find.ts:142-145); cyrup ran both first"
    );
    assert_eq!(pulled.load(Ordering::SeqCst), 0, "no walk entry may be pulled after an abort");
}

/// An abort landing MID-walk must stop the walk on the very next iteration, deterministically.
///
/// RED before: with no loop-top guard and an UNBIASED `select!`, the walk arm won the race against
/// the already-cancelled token with probability ~1/2 per iteration, so `pulled` overshot
/// `cancel_at` — a non-deterministic amount of extra filesystem work after Esc. Repeated 20x here
/// so that pre-fix failure is a certainty (1 - 2^-20) rather than a coin flip; post-fix the loop-top
/// `is_cancelled()` check returns before `select!` is reached at all, so the count is exact.
#[tokio::test]
async fn an_abort_mid_walk_stops_the_walk_on_the_next_iteration() {
    const CANCEL_AT: usize = 3;
    let dir = tree_of(200);

    for attempt in 0..20 {
        let cancel = CancelToken::new();
        let metadata_calls = Arc::new(AtomicUsize::new(0));
        let pulled = Arc::new(AtomicUsize::new(0));
        let fs = Arc::new(AbortProbeFs {
            inner: LocalFs,
            metadata_calls,
            pulled: Arc::clone(&pulled),
            cancel_at: CANCEL_AT,
            token: cancel.clone(),
        });

        let find = FindTool::new(fs, dir.path().to_path_buf(), FindOpts::default());
        let err = find
            .execute(cid(), serde_json::json!({ "pattern": "*.txt" }), cancel, noop_sink())
            .await
            .expect_err("a find aborted mid-walk must reject");

        assert_eq!(err.to_string(), "Operation aborted");
        assert_eq!(
            pulled.load(Ordering::SeqCst),
            CANCEL_AT,
            "attempt {attempt}: the walk must stop at the entry that fired the abort, not keep \
             consuming buffered entries (200-entry tree)"
        );
    }
}
