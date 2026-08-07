//! TOOL-019 — the file-mutation lock must be **process-wide**, not per-`ToolRegistry`.
//!
//! `crates/cyrup-tools/src/lock.rs` ports
//! `pi/packages/coding-agent/src/core/tools/file-mutation-queue.ts`, whose `fileMutationQueues` map
//! lives at MODULE scope (`:4`) behind an exported free function (`:32`) — one map per *process*.
//! Cyrup's `ToolRegistry::with_builtins` constructs the `FileMutationLocks` it hands to `write` and
//! `edit`, and `cyrup-session-svc`'s builder constructs one registry per `AgentSession`, so a
//! per-registry map means two sessions in one process mutate the same file with NO exclusion.
//!
//! That is not a mere loss of atomicity. `FsOps::write_in_place` truncates at `open` and then
//! writes (TOOL-004 — it replaced a temp-file + `rename(2)` dance that had been accidentally
//! capping the damage at "last writer wins, file intact"), so two unserialized mutators interleave
//! their chunks: the file ends up matching NEITHER payload, and both tool calls return success.
//! Silent corruption. These tests pin the exclusion at the registry boundary, where the bug lived.
//!
//! The `write_in_place` used here is a faithful stand-in for the real one — same `create_dir_all`,
//! same `O_WRONLY|O_CREAT|O_TRUNC`, same handle — split into two `write(2)`s with a gap between
//! them. `tokio::fs::File` already chunks any payload past its internal buffer, so this is the
//! shape a large real write takes; the explicit gap only makes the window deterministic instead of
//! leaving detection to the scheduler.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_core::{
    CancelToken, Content, EventStream, Tool, ToolCallId, ToolError, ToolResult, ToolUpdate,
    ToolUpdateSink,
};
use cyrup_tools::ops::local::LocalFs;
use cyrup_tools::ops::{Access, Backend, DirEntry, FsOps, Meta, WalkItem, WalkOpts};
use cyrup_tools::{ToolRegistry, ToolsOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

fn cid() -> ToolCallId {
    ToolCallId::from("tc-xreg")
}

fn noop_sink() -> ToolUpdateSink {
    Box::new(|_u: ToolUpdate| {})
}

fn text_of(r: &ToolResult) -> String {
    r.content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// Highest number of `write_in_place` calls ever in flight simultaneously, across every registry.
#[derive(Default)]
struct Probe {
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
}

/// `LocalFs` with a two-chunk `write_in_place` and a scheduling gap in the middle.
struct SplitWriteFs {
    inner: Arc<dyn FsOps>,
    probe: Arc<Probe>,
    gap: Duration,
}

impl SplitWriteFs {
    fn new(probe: Arc<Probe>, gap: Duration) -> Self {
        Self { inner: Arc::new(LocalFs), probe, gap }
    }
}

#[async_trait::async_trait]
impl FsOps for SplitWriteFs {
    async fn read(&self, path: &Path) -> Result<Vec<u8>, ToolError> {
        self.inner.read(path).await
    }

    async fn write_in_place(&self, path: &Path, bytes: &[u8]) -> Result<(), ToolError> {
        let now = self.probe.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.probe.max_in_flight.fetch_max(now, Ordering::SeqCst);
        let out = self.split_write(path, bytes).await;
        self.probe.in_flight.fetch_sub(1, Ordering::SeqCst);
        out
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

impl SplitWriteFs {
    async fn split_write(&self, path: &Path, bytes: &[u8]) -> Result<(), ToolError> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ToolError::new(format!("create dir: {e}")))?;
        }
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .await
            .map_err(|e| ToolError::new(format!("open: {e}")))?;
        let mid = bytes.len() / 2;
        file.write_all(&bytes[..mid]).await.map_err(|e| ToolError::new(format!("write: {e}")))?;
        file.flush().await.map_err(|e| ToolError::new(format!("flush: {e}")))?;
        tokio::time::sleep(self.gap).await;
        file.write_all(&bytes[mid..]).await.map_err(|e| ToolError::new(format!("write: {e}")))?;
        file.flush().await.map_err(|e| ToolError::new(format!("flush: {e}")))?;
        Ok(())
    }
}

/// Two registries built exactly as `cyrup-session-svc` builds one per session, over one shared
/// instrumented backend so the probe sees both.
fn two_registries(cwd: &Path, probe: Arc<Probe>) -> (ToolRegistry, ToolRegistry) {
    let fs: Arc<dyn FsOps> = Arc::new(SplitWriteFs::new(probe, Duration::from_millis(80)));
    let backend = Backend { fs, ..Backend::default() };
    let a = ToolRegistry::with_builtins(cwd.to_path_buf(), backend.clone(), ToolsOptions::default());
    let b = ToolRegistry::with_builtins(cwd.to_path_buf(), backend, ToolsOptions::default());
    (a, b)
}

/// Byte-level corruption: two `write`s, from two independent registries, to one path.
///
/// The payloads have DIFFERENT LENGTHS on purpose — two equal-length writes can interleave freely
/// and still land on a file of the right size, which would make the content assertion vacuous.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_registries_serialize_writes_to_the_same_file() {
    let dir = tempfile::tempdir().unwrap();
    let cwd: PathBuf = dir.path().to_path_buf();
    let probe = Arc::new(Probe::default());
    let (reg_a, reg_b) = two_registries(&cwd, probe.clone());

    let write_a = reg_a.get("write").expect("write is a built-in");
    let write_b = reg_b.get("write").expect("write is a built-in");
    assert!(!Arc::ptr_eq(&write_a, &write_b), "the two registries must be genuinely independent");

    let payload_a = "A".repeat(4096);
    let payload_b = "B".repeat(6144);

    let ta = {
        let (t, p) = (write_a, payload_a.clone());
        tokio::spawn(async move {
            t.execute(
                cid(),
                serde_json::json!({ "path": "race.txt", "content": p }),
                CancelToken::new(),
                noop_sink(),
            )
            .await
        })
    };
    let tb = {
        let (t, p) = (write_b, payload_b.clone());
        tokio::spawn(async move {
            t.execute(
                cid(),
                serde_json::json!({ "path": "race.txt", "content": p }),
                CancelToken::new(),
                noop_sink(),
            )
            .await
        })
    };
    ta.await.unwrap().expect("write A must succeed");
    tb.await.unwrap().expect("write B must succeed");

    assert_eq!(
        probe.max_in_flight.load(Ordering::SeqCst),
        1,
        "two ToolRegistry instances entered write_in_place on the same path at once — the lock map \
         is not process-global (pi keeps ONE module-scope map, file-mutation-queue.ts:4)"
    );

    let final_text = std::fs::read_to_string(cwd.join("race.txt")).unwrap();
    assert!(
        final_text == payload_a || final_text == payload_b,
        "interleaved write: {} bytes, {} 'A' and {} 'B' — expected exactly one whole payload \
         ({} or {} bytes)",
        final_text.len(),
        final_text.chars().filter(|c| *c == 'A').count(),
        final_text.chars().filter(|c| *c == 'B').count(),
        payload_a.len(),
        payload_b.len(),
    );
}

/// Semantic corruption: `edit` is a read-modify-write, so an unserialized pair loses an update even
/// when neither write tears. Both edits target the same unique anchor, so a correctly serialized
/// run leaves BOTH markers in the file whichever order they run in; a lost update drops one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_registries_do_not_lose_an_edit_to_the_same_file() {
    let dir = tempfile::tempdir().unwrap();
    let cwd: PathBuf = dir.path().to_path_buf();
    std::fs::write(cwd.join("notes.txt"), "SEED\n").unwrap();

    let probe = Arc::new(Probe::default());
    let (reg_a, reg_b) = two_registries(&cwd, probe.clone());
    let edit_a = reg_a.get("edit").expect("edit is a built-in");
    let edit_b = reg_b.get("edit").expect("edit is a built-in");

    let spawn_edit = |t: Arc<dyn Tool>, marker: &'static str| {
        tokio::spawn(async move {
            t.execute(
                cid(),
                serde_json::json!({
                    "path": "notes.txt",
                    "edits": [{ "oldText": "SEED", "newText": format!("SEED\n{marker}") }],
                }),
                CancelToken::new(),
                noop_sink(),
            )
            .await
        })
    };

    let ta = spawn_edit(edit_a, "ALPHA");
    let tb = spawn_edit(edit_b, "BETA");
    let ra = ta.await.unwrap();
    let rb = tb.await.unwrap();

    let ra = ra.unwrap_or_else(|e| panic!("edit ALPHA must succeed, got: {e}"));
    let rb = rb.unwrap_or_else(|e| panic!("edit BETA must succeed, got: {e}"));
    assert!(text_of(&ra).contains("Successfully replaced"));
    assert!(text_of(&rb).contains("Successfully replaced"));

    assert_eq!(
        probe.max_in_flight.load(Ordering::SeqCst),
        1,
        "two ToolRegistry instances mutated the same path concurrently"
    );

    let final_text = std::fs::read_to_string(cwd.join("notes.txt")).unwrap();
    assert!(
        final_text.contains("ALPHA") && final_text.contains("BETA"),
        "lost update — the second edit read the file before the first finished writing it. \
         Final content was {final_text:?}"
    );
}

/// The counterweight: one process-global map must NOT collapse unrelated files onto one lock.
/// Pi is explicit that "Operations for different files still run in parallel"
/// (file-mutation-queue.ts:30). With an 80 ms gap per write, four serialized writes would take
/// >=320 ms; parallel ones finish in roughly one gap.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn distinct_files_across_registries_still_run_in_parallel() {
    let dir = tempfile::tempdir().unwrap();
    let cwd: PathBuf = dir.path().to_path_buf();
    let probe = Arc::new(Probe::default());
    let (reg_a, reg_b) = two_registries(&cwd, probe.clone());

    let mut handles = Vec::new();
    for (reg, n) in [(&reg_a, 0), (&reg_a, 1), (&reg_b, 2), (&reg_b, 3)] {
        let t = reg.get("write").expect("write is a built-in");
        handles.push(tokio::spawn(async move {
            t.execute(
                cid(),
                serde_json::json!({ "path": format!("file-{n}.txt"), "content": format!("v{n}") }),
                CancelToken::new(),
                noop_sink(),
            )
            .await
        }));
    }
    for h in handles {
        h.await.unwrap().expect("write must succeed");
    }

    assert!(
        probe.max_in_flight.load(Ordering::SeqCst) > 1,
        "distinct paths serialized against each other — the per-path keying was lost"
    );
    for n in 0..4 {
        let got = std::fs::read_to_string(cwd.join(format!("file-{n}.txt"))).unwrap();
        assert_eq!(got, format!("v{n}"));
    }
}
