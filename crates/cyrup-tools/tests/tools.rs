//! Integration tests for the built-in tools (A-03-1…9). Real filesystem (tempdir fixtures) and a
//! real `bash`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use cyrup_core::{
    CancelToken, Content, ExecMode, Tool, ToolCallId, ToolError, ToolResult, ToolUpdate,
    ToolUpdateSink,
};
use cyrup_tools::config::{BashOpts, FindOpts, GrepOpts, LsOpts, ReadOpts};
use cyrup_tools::ops::local::LocalFs;
use cyrup_tools::ops::{Backend, FsOps, ProcOps, ShellConfig};
use cyrup_tools::registry::{Availability, ToolRegistry};
use cyrup_tools::tools::{BashTool, EditTool, FindTool, GrepTool, LsTool, ReadTool, WriteTool};
use cyrup_tools::{FileMutationLocks, ToolsOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn fs() -> Arc<dyn FsOps> {
    Arc::new(LocalFs)
}

fn proc() -> Arc<dyn ProcOps> {
    Backend::default().proc
}

fn cid() -> ToolCallId {
    ToolCallId::from("tc-test")
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

// ---------------------------------------------------------------- A-03-1 read

#[tokio::test]
async fn read_window_and_truncation_and_oversized() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    let big = (1..=10).map(|i| format!("line{i}")).collect::<Vec<_>>().join("\n");
    std::fs::write(cwd.join("f.txt"), &big).unwrap();

    // Window: offset=2 limit=3 -> lines 2,3,4.
    let read = ReadTool::new(fs(), cwd.clone(), ReadOpts::default());
    let r = read
        .execute(
            cid(),
            serde_json::json!({ "path": "f.txt", "offset": 2, "limit": 3 }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    let text = first_text(&r);
    assert!(text.starts_with("line2\nline3\nline4"), "got: {text}");

    // Truncation by lines with notice.
    let read_small = ReadTool::new(
        fs(),
        cwd.clone(),
        ReadOpts { max_lines: 3, ..ReadOpts::default() },
    );
    let r = read_small
        .execute(cid(), serde_json::json!({ "path": "f.txt" }), CancelToken::new(), noop_sink())
        .await
        .unwrap();
    let text = first_text(&r);
    assert!(text.contains("line1\nline2\nline3"));
    assert!(text.contains("Showing lines 1-3 of 10"), "got: {text}");

    // Oversized single line -> bash fallback error.
    std::fs::write(cwd.join("long.txt"), "x".repeat(200)).unwrap();
    let read_tiny =
        ReadTool::new(fs(), cwd.clone(), ReadOpts { max_bytes: 50, ..ReadOpts::default() });
    let err = read_tiny
        .execute(cid(), serde_json::json!({ "path": "long.txt" }), CancelToken::new(), noop_sink())
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("exceeds") && msg.contains("bash"), "got: {msg}");
}

#[tokio::test]
async fn read_missing_file_errors() {
    let dir = tempfile::tempdir().unwrap();
    let read = ReadTool::new(fs(), dir.path().to_path_buf(), ReadOpts::default());
    let err = read
        .execute(cid(), serde_json::json!({ "path": "nope.txt" }), CancelToken::new(), noop_sink())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not found") || err.to_string().contains("unreadable"));
}

// ---------------------------------------------------------------- A-03-2 image

#[tokio::test]
async fn read_image_non_vision_warns() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    std::fs::write(cwd.join("pic.png"), b"\x89PNG not-really").unwrap();
    let read = ReadTool::new(
        fs(),
        cwd,
        ReadOpts { supports_images: false, ..ReadOpts::default() },
    );
    let r = read
        .execute(cid(), serde_json::json!({ "path": "pic.png" }), CancelToken::new(), noop_sink())
        .await
        .unwrap();
    let text = first_text(&r);
    assert!(text.contains("does not support images"), "got: {text}");
    assert!(!r.content.iter().any(|c| matches!(c, Content::Image { .. })));
}

#[cfg(feature = "inline-images")]
#[tokio::test]
async fn read_image_vision_attaches() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    // Generate a valid 2x2 PNG via the image crate.
    let img = image::RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 255]));
    img.save(cwd.join("p.png")).unwrap();
    let read = ReadTool::new(fs(), cwd, ReadOpts::default());
    let r = read
        .execute(cid(), serde_json::json!({ "path": "p.png" }), CancelToken::new(), noop_sink())
        .await
        .unwrap();
    assert!(r.content.iter().any(|c| matches!(c, Content::Image { .. })));
}

// ---------------------------------------------------------------- A-03-3 edit

fn edit_tool(cwd: PathBuf) -> EditTool {
    EditTool::new(fs(), Arc::new(FileMutationLocks::new()), cwd, Default::default())
}

#[tokio::test]
async fn edit_unique_crlf_bom_diff() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    // BOM + CRLF.
    let original = "\u{feff}one\r\ntwo\r\nthree\r\n";
    std::fs::write(cwd.join("f.txt"), original.as_bytes()).unwrap();

    let edit = edit_tool(cwd.clone());
    let r = edit
        .execute(
            cid(),
            serde_json::json!({
                "path": "f.txt",
                "edits": [{ "oldText": "two", "newText": "TWO" }]
            }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    assert!(first_text(&r).contains("replaced 1 block"));

    let after = std::fs::read(cwd.join("f.txt")).unwrap();
    let after = String::from_utf8(after).unwrap();
    assert_eq!(after, "\u{feff}one\r\nTWO\r\nthree\r\n", "CRLF+BOM preserved");

    let details = r.details.unwrap();
    assert!(details["diff"].as_str().unwrap().contains("+TWO"));
    assert!(details["patch"].as_str().unwrap().contains("@@"));
    assert_eq!(details["firstChangedLine"], 2);
}

#[tokio::test]
async fn edit_non_unique_errors() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    std::fs::write(cwd.join("f.txt"), "dup\ndup\n").unwrap();
    let edit = edit_tool(cwd);
    let err = edit
        .execute(
            cid(),
            serde_json::json!({ "path": "f.txt", "edits": [{ "oldText": "dup", "newText": "x" }] }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not unique"));
}

#[tokio::test]
async fn edit_legacy_single_and_stringified_shims() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    std::fs::write(cwd.join("a.txt"), "alpha\n").unwrap();
    std::fs::write(cwd.join("b.txt"), "beta\n").unwrap();
    let edit = edit_tool(cwd.clone());

    // legacy top-level oldText/newText
    edit.execute(
        cid(),
        serde_json::json!({ "path": "a.txt", "oldText": "alpha", "newText": "ALPHA" }),
        CancelToken::new(),
        noop_sink(),
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read_to_string(cwd.join("a.txt")).unwrap(), "ALPHA\n");

    // edits sent as a JSON string
    edit.execute(
        cid(),
        serde_json::json!({ "path": "b.txt", "edits": "[{\"oldText\":\"beta\",\"newText\":\"BETA\"}]" }),
        CancelToken::new(),
        noop_sink(),
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read_to_string(cwd.join("b.txt")).unwrap(), "BETA\n");
}

// ---------------------------------------------------------------- A-03-4 write

#[tokio::test]
async fn write_creates_dirs_and_serializes() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    let locks = Arc::new(FileMutationLocks::new());

    let write = WriteTool::new(fs(), locks.clone(), cwd.clone(), Default::default());
    let r = write
        .execute(
            cid(),
            serde_json::json!({ "path": "nested/deep/f.txt", "content": "hello" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    assert!(first_text(&r).contains("Wrote 5 bytes"));
    assert_eq!(std::fs::read_to_string(cwd.join("nested/deep/f.txt")).unwrap(), "hello");

    // Concurrent writes to the same path: no corruption (final == one full content).
    let w = Arc::new(WriteTool::new(fs(), locks, cwd.clone(), Default::default()));
    let a = {
        let w = w.clone();
        tokio::spawn(async move {
            w.execute(
                cid(),
                serde_json::json!({ "path": "race.txt", "content": "AAAA" }),
                CancelToken::new(),
                noop_sink(),
            )
            .await
        })
    };
    let b = {
        let w = w.clone();
        tokio::spawn(async move {
            w.execute(
                cid(),
                serde_json::json!({ "path": "race.txt", "content": "BBBB" }),
                CancelToken::new(),
                noop_sink(),
            )
            .await
        })
    };
    a.await.unwrap().unwrap();
    b.await.unwrap().unwrap();
    let final_content = std::fs::read_to_string(cwd.join("race.txt")).unwrap();
    assert!(final_content == "AAAA" || final_content == "BBBB", "got: {final_content}");
}

// ---------------------------------------------------------------- A-03-5 bash

fn bash_tool(cwd: PathBuf, opts: BashOpts) -> BashTool {
    BashTool::new(proc(), ShellConfig::detect(), cwd, opts)
}

#[tokio::test]
async fn bash_streams_and_exit_zero() {
    let dir = tempfile::tempdir().unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let sink: ToolUpdateSink = {
        let count = count.clone();
        Box::new(move |_u| {
            count.fetch_add(1, Ordering::SeqCst);
        })
    };
    let bash = bash_tool(dir.path().to_path_buf(), BashOpts::default());
    let r = bash
        .execute(
            cid(),
            serde_json::json!({ "command": "echo hello" }),
            CancelToken::new(),
            sink,
        )
        .await
        .unwrap();
    assert!(first_text(&r).contains("hello"));
    assert!(count.load(Ordering::SeqCst) >= 1, "expected >=1 update");
}

#[tokio::test]
async fn bash_nonzero_exit_throws_with_output() {
    let dir = tempfile::tempdir().unwrap();
    let bash = bash_tool(dir.path().to_path_buf(), BashOpts::default());
    let err = bash
        .execute(
            cid(),
            serde_json::json!({ "command": "echo boom; exit 3" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("boom"), "output included: {msg}");
    assert!(msg.contains("exited with code 3"), "got: {msg}");
}

#[tokio::test]
async fn bash_timeout_kills() {
    let dir = tempfile::tempdir().unwrap();
    let bash = bash_tool(dir.path().to_path_buf(), BashOpts::default());
    let err = bash
        .execute(
            cid(),
            serde_json::json!({ "command": "sleep 30", "timeout": 1 }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("timed out"), "got: {}", err);
}

#[tokio::test]
async fn bash_abort_kills() {
    let dir = tempfile::tempdir().unwrap();
    let bash = Arc::new(bash_tool(dir.path().to_path_buf(), BashOpts::default()));
    let cancel = CancelToken::new();
    let task = {
        let bash = bash.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move {
            bash.execute(
                cid(),
                serde_json::json!({ "command": "sleep 30" }),
                cancel,
                noop_sink(),
            )
            .await
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    cancel.cancel();
    let res = task.await.unwrap();
    assert!(res.unwrap_err().to_string().contains("aborted"));
}

#[tokio::test]
async fn bash_truncation_spills_to_temp_file() {
    let dir = tempfile::tempdir().unwrap();
    let bash = bash_tool(
        dir.path().to_path_buf(),
        BashOpts { max_lines: 5, max_bytes: 100, command_prefix: None },
    );
    let r = bash
        .execute(
            cid(),
            serde_json::json!({ "command": "for i in $(seq 1 200); do echo line$i; done" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    assert!(first_text(&r).contains("Full output:"));
    let details = r.details.unwrap();
    let path = details["fullOutputPath"].as_str().expect("full output path");
    let full = std::fs::read_to_string(path).unwrap();
    assert!(full.contains("line1\n") && full.contains("line200"));
}

// ---------------------------------------------------------------- A-03-6 / A-03-9

fn fixture_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    std::fs::write(p.join("a.txt"), "hello world\nsecond line\n").unwrap();
    std::fs::write(p.join("b.log"), "hello from log\n").unwrap();
    std::fs::create_dir(p.join("sub")).unwrap();
    std::fs::write(p.join("sub/c.txt"), "hello nested\n").unwrap();
    std::fs::write(p.join(".gitignore"), "*.log\n").unwrap();
    dir
}

#[tokio::test]
async fn grep_format_and_gitignore_and_no_matches() {
    let dir = fixture_repo();
    let cwd = dir.path().to_path_buf();
    let grep = GrepTool::new(fs(), cwd.clone(), GrepOpts::default());

    let r = grep
        .execute(cid(), serde_json::json!({ "pattern": "hello" }), CancelToken::new(), noop_sink())
        .await
        .unwrap();
    let text = first_text(&r);
    // documented shape: filepath:line: match
    assert!(text.contains("a.txt:1: hello world"), "got: {text}");
    assert!(text.contains("sub/c.txt:1: hello nested"), "got: {text}");
    // gitignored *.log excluded
    assert!(!text.contains("b.log"), "gitignore not respected: {text}");

    let none = grep
        .execute(cid(), serde_json::json!({ "pattern": "zzz_nomatch" }), CancelToken::new(), noop_sink())
        .await
        .unwrap();
    assert_eq!(first_text(&none), "No matches found");
}

#[tokio::test]
async fn find_format_and_gitignore_and_sentinel() {
    let dir = fixture_repo();
    let cwd = dir.path().to_path_buf();
    let find = FindTool::new(fs(), cwd.clone(), FindOpts::default());

    let r = find
        .execute(cid(), serde_json::json!({ "pattern": "*.txt" }), CancelToken::new(), noop_sink())
        .await
        .unwrap();
    let text = first_text(&r);
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines.contains(&"a.txt"), "got: {text}");
    assert!(lines.contains(&"sub/c.txt"), "got: {text}");

    // gitignored log not found
    let none = find
        .execute(cid(), serde_json::json!({ "pattern": "*.log" }), CancelToken::new(), noop_sink())
        .await
        .unwrap();
    assert_eq!(first_text(&none), "No files found matching pattern");

    // directory suffix '/'
    let dirs = find
        .execute(cid(), serde_json::json!({ "pattern": "sub" }), CancelToken::new(), noop_sink())
        .await
        .unwrap();
    assert!(first_text(&dirs).contains("sub/"), "got: {}", first_text(&dirs));
}

#[tokio::test]
async fn find_limit_truncation() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    for i in 0..10 {
        std::fs::write(cwd.join(format!("f{i}.txt")), "x").unwrap();
    }
    let find = FindTool::new(fs(), cwd, FindOpts { limit: 2, max_bytes: 50 * 1024 });
    let r = find
        .execute(cid(), serde_json::json!({ "pattern": "*.txt" }), CancelToken::new(), noop_sink())
        .await
        .unwrap();
    assert!(first_text(&r).contains("result limit reached"), "got: {}", first_text(&r));
    assert_eq!(r.details.unwrap()["resultLimitReached"], 2);
}

#[tokio::test]
async fn ls_sorted_dotfiles_dirs_and_errors() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    std::fs::write(cwd.join("B.txt"), "").unwrap();
    std::fs::write(cwd.join("a.txt"), "").unwrap();
    std::fs::write(cwd.join(".dot"), "").unwrap();
    std::fs::create_dir(cwd.join("zdir")).unwrap();
    let ls = LsTool::new(fs(), cwd.clone(), LsOpts::default());

    let r = ls
        .execute(cid(), serde_json::json!({}), CancelToken::new(), noop_sink())
        .await
        .unwrap();
    let text = first_text(&r);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines, vec![".dot", "a.txt", "B.txt", "zdir/"], "case-insensitive sort + '/'");

    // empty dir
    let empty = tempfile::tempdir().unwrap();
    let ls2 = LsTool::new(fs(), empty.path().to_path_buf(), LsOpts::default());
    let r = ls2
        .execute(cid(), serde_json::json!({}), CancelToken::new(), noop_sink())
        .await
        .unwrap();
    assert_eq!(first_text(&r), "(empty directory)");

    // not a directory
    let err = ls
        .execute(cid(), serde_json::json!({ "path": "a.txt" }), CancelToken::new(), noop_sink())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("Not a directory"));
}

// ---------------------------------------------------------------- A-03-7 availability

#[tokio::test]
async fn availability_controls() {
    let dir = tempfile::tempdir().unwrap();
    let reg = ToolRegistry::with_builtins(
        dir.path().to_path_buf(),
        Backend::default(),
        ToolsOptions::default(),
    );
    assert_eq!(reg.all().len(), 7);

    let names = |v: &[Arc<dyn Tool>]| v.iter().map(|t| t.name().to_string()).collect::<Vec<_>>();

    let allow: std::collections::HashSet<String> =
        ["read", "grep"].into_iter().map(String::from).collect();
    let v = reg.visible(&Availability::Allow(allow));
    assert_eq!(names(&v), vec!["read", "grep"]);

    let exclude: std::collections::HashSet<String> =
        ["bash"].into_iter().map(String::from).collect();
    assert!(!names(&reg.visible(&Availability::Exclude(exclude))).contains(&"bash".to_string()));

    assert!(reg.visible(&Availability::NoTools).is_empty());
    assert!(reg.visible(&Availability::NoBuiltins).is_empty()); // only built-ins registered
}

// ---------------------------------------------------------------- A-03-8 override + throw

struct EchoRead {
    params: serde_json::Value,
}
#[async_trait::async_trait]
impl Tool for EchoRead {
    fn name(&self) -> &str {
        "read"
    }
    fn parameters(&self) -> &serde_json::Value {
        &self.params
    }
    fn execution_mode(&self) -> ExecMode {
        ExecMode::Parallel
    }
    async fn execute(
        &self,
        _c: ToolCallId,
        _p: serde_json::Value,
        _cancel: CancelToken,
        _u: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult {
            content: vec![Content::text("overridden read")],
            details: None,
            terminate: false,
        })
    }
}

struct Boom;
#[async_trait::async_trait]
impl Tool for Boom {
    fn name(&self) -> &str {
        "boom"
    }
    fn parameters(&self) -> &serde_json::Value {
        static EMPTY: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
        EMPTY.get_or_init(|| serde_json::json!({ "type": "object" }))
    }
    async fn execute(
        &self,
        _c: ToolCallId,
        _p: serde_json::Value,
        _cancel: CancelToken,
        _u: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        Err(ToolError::new("boom went the tool"))
    }
}

#[tokio::test]
async fn extension_override_and_throwing_tool() {
    let dir = tempfile::tempdir().unwrap();
    let mut reg = ToolRegistry::with_builtins(
        dir.path().to_path_buf(),
        Backend::default(),
        ToolsOptions::default(),
    );
    // Override built-in `read`.
    reg.insert(Arc::new(EchoRead { params: serde_json::json!({ "type": "object" }) }));
    assert_eq!(reg.all().len(), 7, "override does not add a new slot");
    let read = reg.get("read").unwrap();
    let r = read
        .execute(cid(), serde_json::json!({}), CancelToken::new(), noop_sink())
        .await
        .unwrap();
    assert_eq!(first_text(&r), "overridden read");

    // Throwing tool -> Err (mapped to isError by the runtime).
    reg.insert(Arc::new(Boom));
    let boom = reg.get("boom").unwrap();
    let err = boom
        .execute(cid(), serde_json::json!({}), CancelToken::new(), noop_sink())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("boom"));
}

// ---------------------------------------------------------------- seam re-target

struct CountingFs {
    inner: Arc<dyn FsOps>,
    reads: Arc<AtomicUsize>,
}
#[async_trait::async_trait]
impl FsOps for CountingFs {
    async fn read(&self, path: &Path) -> Result<Vec<u8>, ToolError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.inner.read(path).await
    }
    async fn write_atomic(&self, path: &Path, bytes: &[u8]) -> Result<(), ToolError> {
        self.inner.write_atomic(path, bytes).await
    }
    async fn access(&self, path: &Path, mode: cyrup_tools::ops::Access) -> Result<(), ToolError> {
        self.inner.access(path, mode).await
    }
    async fn metadata(&self, path: &Path) -> Result<cyrup_tools::ops::Meta, ToolError> {
        self.inner.metadata(path).await
    }
    async fn read_dir(&self, path: &Path) -> Result<Vec<cyrup_tools::ops::DirEntry>, ToolError> {
        self.inner.read_dir(path).await
    }
    fn walk(
        &self,
        root: &Path,
        opts: cyrup_tools::ops::WalkOpts,
    ) -> cyrup_core::EventStream<Result<cyrup_tools::ops::WalkItem, ToolError>> {
        self.inner.walk(root, opts)
    }
}

#[tokio::test]
async fn tool_logic_is_backend_agnostic() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    std::fs::write(cwd.join("f.txt"), "data\n").unwrap();
    let reads = Arc::new(AtomicUsize::new(0));
    let counting: Arc<dyn FsOps> = Arc::new(CountingFs { inner: fs(), reads: reads.clone() });
    let read = ReadTool::new(counting, cwd, ReadOpts::default());
    let r = read
        .execute(cid(), serde_json::json!({ "path": "f.txt" }), CancelToken::new(), noop_sink())
        .await
        .unwrap();
    assert!(first_text(&r).contains("data"));
    assert!(reads.load(Ordering::SeqCst) >= 1, "tool routed through the seam");
}
