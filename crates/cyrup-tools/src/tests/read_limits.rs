//! `read` must not take an unbounded amount of host memory for one call.
//!
//! The nested `tools.read` of a codemode script and a direct `read` reach the same tool, and the
//! sandbox's memory limits bound the script, not the host work its calls trigger. Measured on the
//! real binary before this change: `read /dev/zero` took the host from 740 MB to 4 GB in four
//! seconds, and a sparse 3 GiB regular file took it to 5.8 GB (the bytes, then the same again as
//! text). Node's `readFile`, which Pi calls, refuses a regular file over 2 GiB with
//! `ERR_FS_FILE_TOO_LARGE` and cannot build a text string past `0x1fffffe8` characters.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use crate::config::ReadOpts;
use crate::ops::local::LocalFs;
use crate::ops::{Access, DirEntry, FsOps, Meta, WalkItem, WalkOpts};
use crate::tools::ReadTool;
use cyrup_core::{
    CancelToken, Content, EventStream, Tool, ToolCallId, ToolError, ToolResult, ToolUpdate,
};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

async fn run(tool: &ReadTool, path: &str) -> Result<ToolResult, ToolError> {
    tool.execute(
        ToolCallId::from("tc-limits"),
        serde_json::json!({ "path": path }),
        CancelToken::new(),
        Box::new(|_u: ToolUpdate| {}),
    )
    .await
}

fn text(result: &ToolResult) -> String {
    match result.content.first() {
        Some(Content::Text { text, .. }) => text.to_string(),
        _ => String::new(),
    }
}

fn local_tool(cwd: &Path, limit: u64) -> ReadTool {
    ReadTool::new(Arc::new(LocalFs), cwd.to_path_buf(), ReadOpts::default())
        .with_max_read_bytes(limit)
}

/// A backend whose `metadata` and `read_stream` are the test's to script, over a real `LocalFs`
/// for everything else.
struct Scripted {
    inner: LocalFs,
    /// `None` makes `metadata` fail, as a remote filesystem without it would.
    meta: Option<Meta>,
    /// Bytes `read_stream` yields, however long the real file is.
    stream_len: usize,
    streams_opened: AtomicUsize,
}

impl Scripted {
    fn new(meta: Option<Meta>, stream_len: usize) -> Self {
        Self {
            inner: LocalFs,
            meta,
            stream_len,
            streams_opened: AtomicUsize::new(0),
        }
    }
}

#[async_trait::async_trait]
impl FsOps for Scripted {
    async fn read(&self, path: &Path) -> Result<Vec<u8>, ToolError> {
        self.inner.read(path).await
    }
    async fn read_stream(&self, _path: &Path) -> Result<Box<dyn std::io::Read + Send>, ToolError> {
        self.streams_opened.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(std::io::repeat(b'a').take(self.stream_len as u64)))
    }
    async fn write_in_place(&self, path: &Path, bytes: &[u8]) -> Result<(), ToolError> {
        self.inner.write_in_place(path, bytes).await
    }
    async fn access(&self, path: &Path, mode: Access) -> Result<(), ToolError> {
        self.inner.access(path, mode).await
    }
    async fn metadata(&self, path: &Path) -> Result<Meta, ToolError> {
        self.meta
            .clone()
            .ok_or_else(|| ToolError::new("metadata unavailable on this backend"))
            .map(|m| Meta {
                canonical: path.to_path_buf(),
                ..m
            })
    }
    async fn read_dir(&self, path: &Path) -> Result<Vec<DirEntry>, ToolError> {
        self.inner.read_dir(path).await
    }
    fn walk(&self, root: &Path, opts: WalkOpts) -> EventStream<Result<WalkItem, ToolError>> {
        self.inner.walk(root, opts)
    }
}

fn meta(is_file: bool, is_dir: bool, len: u64) -> Meta {
    Meta {
        is_dir,
        is_file,
        len,
        canonical: PathBuf::new(),
    }
}

fn scripted_tool(cwd: &Path, fs: &Arc<Scripted>, limit: u64) -> ReadTool {
    ReadTool::new(fs.clone(), cwd.to_path_buf(), ReadOpts::default()).with_max_read_bytes(limit)
}

/// A regular file one byte over the limit is refused from its size alone: the message quotes the
/// size `stat` reported, which the bounded drain (that does not know it) cannot.
#[tokio::test]
async fn a_regular_file_over_the_limit_is_refused_by_its_size() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("big.txt"), "x".repeat(11)).unwrap();
    let err = run(&local_tool(dir.path(), 10), "big.txt")
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.starts_with("File size (11) is greater than the "),
        "must quote the size stat reported, got: {msg}"
    );
    assert!(
        msg.contains("Use bash"),
        "must say what to do instead, got: {msg}"
    );
}

/// The limit itself is readable; one byte more is not.
#[tokio::test]
async fn a_regular_file_of_exactly_the_limit_is_read() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("fits.txt"), "y".repeat(10)).unwrap();
    let ok = run(&local_tool(dir.path(), 10), "fits.txt").await.unwrap();
    assert_eq!(text(&ok), "y".repeat(10));
}

/// Node's own message for a regular file over 2 GiB (`ERR_FS_FILE_TOO_LARGE`), checked against
/// Node 22.22.2 with a sparse 2 GiB and a sparse 3 GiB file. Sparse here too, so the test costs no
/// disk and, with the check in place, no read.
#[tokio::test]
async fn a_file_over_two_gib_gets_nodes_message() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("huge.bin");
    let file = std::fs::File::create(&path).unwrap();
    if file.set_len(1 << 31).is_err() {
        return; // the filesystem under the temp dir cannot hold a sparse file this size.
    }
    drop(file);
    let tool = ReadTool::new(
        Arc::new(LocalFs),
        dir.path().to_path_buf(),
        ReadOpts::default(),
    );
    let err = run(&tool, "huge.bin").await.unwrap_err();
    assert_eq!(
        err.to_string(),
        "File size (2147483648) is greater than 2 GiB"
    );
}

/// A device is not a file `read` can ever finish: `/dev/zero` has no end and `/dev/null` is empty
/// only by luck. `/dev/null` is the one that is safe to run unguarded, which is what lets the
/// red proof of this test fail by returning `Ok` instead of by exhausting memory.
#[cfg(unix)]
#[tokio::test]
async fn a_character_device_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let tool = ReadTool::new(
        Arc::new(LocalFs),
        dir.path().to_path_buf(),
        ReadOpts::default(),
    );
    let err = run(&tool, "/dev/null").await.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.starts_with("Cannot read /dev/null: it is not a regular file"),
        "got: {msg}"
    );
}

/// The refusal comes before the stream is opened: opening a FIFO blocks until a writer shows up.
#[tokio::test]
async fn a_special_file_is_refused_before_the_stream_is_opened() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("pipe"), "x").unwrap();
    let fs = Arc::new(Scripted::new(Some(meta(false, false, 0)), 1));
    let err = run(&scripted_tool(dir.path(), &fs, 1000), "pipe")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not a regular file"), "got: {err}");
    assert_eq!(fs.streams_opened.load(Ordering::SeqCst), 0);
}

/// A directory still fails at the read, with its errno text, as before.
#[tokio::test]
async fn a_directory_is_not_called_a_special_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    let err = run(&local_tool(dir.path(), 1000), "sub").await.unwrap_err();
    let msg = err.to_string();
    assert!(!msg.contains("not a regular file"), "got: {msg}");
}

/// A backend that cannot `stat` is not a reason to fail the read: `access` already vouched for the
/// path and the drain enforces the limit on what arrives.
#[tokio::test]
async fn a_backend_without_metadata_still_reads() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.txt"), "x").unwrap();
    let fs = Arc::new(Scripted::new(None, 5));
    let ok = run(&scripted_tool(dir.path(), &fs, 1000), "f.txt")
        .await
        .unwrap();
    assert_eq!(text(&ok), "aaaaa");
}

/// `stat` said the file is small (a `/proc` file says 0; a log another process keeps appending to
/// says what it was a moment ago) and the stream is longer: the drain stops at the limit.
#[tokio::test]
async fn a_stream_longer_than_stat_said_stops_at_the_limit() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("grows.txt"), "x").unwrap();
    // 2 MiB of stream against a 1 MiB limit; the stat answer (0) hides it.
    let fs = Arc::new(Scripted::new(Some(meta(true, false, 0)), 2 << 20));
    let err = run(&scripted_tool(dir.path(), &fs, 1 << 20), "grows.txt")
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.starts_with("File size is greater than the 1.0MB"),
        "got: {msg}"
    );
    assert_eq!(fs.streams_opened.load(Ordering::SeqCst), 1);
}

/// Bytes that are not UTF-8 still become text (U+FFFD for each bad byte), as `buffer.toString("utf-8")`
/// does, now that valid text is taken over without a copy.
#[tokio::test]
async fn invalid_utf8_still_reads_lossily() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("bad.txt"), [b'a', 0xff, b'b', b'\n', b'c']).unwrap();
    let ok = run(&local_tool(dir.path(), 1000), "bad.txt").await.unwrap();
    assert_eq!(text(&ok), "a\u{fffd}b\nc");
}
