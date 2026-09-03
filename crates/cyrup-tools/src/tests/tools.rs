//! Integration tests for the built-in tools (A-03-1…9). Real filesystem (tempdir fixtures) and a
//! real `bash`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use crate::config::{BashOpts, FindOpts, GrepOpts, LsOpts, PowerShellOpts, ReadOpts};
use crate::ops::local::LocalFs;
use crate::ops::{Backend, ExecSpec, ExitStatus, FsOps, ProcOps};
use crate::registry::{Availability, ToolRegistry};
use crate::tools::{EditTool, FindTool, GrepTool, LsTool, ReadTool, ShellTool, WriteTool};
use crate::{FileMutationLocks, ToolsOptions};
use cyrup_core::{
    CancelToken, Content, ExecMode, Tool, ToolCallId, ToolError, ToolResult, ToolUpdate,
    ToolUpdateSink,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

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
            return text.to_string();
        }
    }
    String::new()
}

// ---------------------------------------------------------------- A-03-1 read

#[tokio::test]
async fn read_window_and_truncation_and_oversized() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    let big = (1..=10)
        .map(|i| format!("line{i}"))
        .collect::<Vec<_>>()
        .join("\n");
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
        ReadOpts {
            max_lines: 3,
            ..ReadOpts::default()
        },
    );
    let r = read_small
        .execute(
            cid(),
            serde_json::json!({ "path": "f.txt" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    let text = first_text(&r);
    assert!(text.contains("line1\nline2\nline3"));
    assert!(text.contains("Showing lines 1-3 of 10"), "got: {text}");

    // Oversized single line -> Pi resolves SUCCESSFULLY with a bash-fallback content note
    // (read.ts:290-294,315), not an error. Details carry the truncation.
    std::fs::write(cwd.join("long.txt"), "x".repeat(200)).unwrap();
    let read_tiny = ReadTool::new(
        fs(),
        cwd.clone(),
        ReadOpts {
            max_bytes: 50,
            ..ReadOpts::default()
        },
    );
    let r = read_tiny
        .execute(
            cid(),
            serde_json::json!({ "path": "long.txt" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    let msg = first_text(&r);
    assert!(
        msg.contains("exceeds") && msg.contains("bash"),
        "got: {msg}"
    );
    assert!(msg.contains("50.0KB") || msg.contains("50B"), "got: {msg}");
    assert!(
        r.details.is_some(),
        "first-line-exceeds must attach truncation details"
    );
}

// A non-truncated read returns the file content VERBATIM, preserving the trailing newline — Pi's
// `truncateHead` short-circuits and returns the input unchanged (truncate.ts:87-101). Previously
// cyrup rebuilt from split lines and dropped the final `\n`, a pervasive byte divergence on `read`.
#[tokio::test]
async fn read_preserves_trailing_newline_verbatim() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    std::fs::write(cwd.join("nl.txt"), "hello\nworld\n").unwrap();
    let read = ReadTool::new(fs(), cwd, ReadOpts::default());
    let r = read
        .execute(
            cid(),
            serde_json::json!({ "path": "nl.txt" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    let text = first_text(&r);
    assert_eq!(
        text, "hello\nworld\n",
        "trailing newline must be preserved verbatim"
    );
    assert!(r.details.is_none(), "no truncation -> no details");
}

#[tokio::test]
async fn read_missing_file_errors() {
    let dir = tempfile::tempdir().unwrap();
    let read = ReadTool::new(fs(), dir.path().to_path_buf(), ReadOpts::default());
    let err = read
        .execute(
            cid(),
            serde_json::json!({ "path": "nope.txt" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap_err();
    // Pi re-rejects Node's raw `fs.access` error (read.ts:241/321-324), so the message carries the
    // errno and the resolved absolute path — see `tests/read_access_errno.rs`.
    let msg = err.to_string();
    assert!(msg.contains("nope.txt"), "got: {msg}");
    // Asserted on the errno CODE, which `errno_name` produces on BOTH `access` arms (error.rs);
    // the trailing `strerror` prose is unix-only and made this test un-runnable on the Windows
    // arm the crate actually ships.
    assert!(msg.starts_with("ENOENT: "), "got: {msg}");
    #[cfg(unix)]
    assert!(msg.contains("No such file or directory"), "got: {msg}");
}

// ---------------------------------------------------------------- A-03-2 image

// Pi (read.ts:246-263) STILL returns the image block for a non-vision model — together with the
// warning note — and the request layer strips it later. Detection is by MAGIC BYTES, so the
// fixture must be a real PNG (an invalid header would be read as text).
#[cfg(feature = "inline-images")]
#[tokio::test]
async fn read_image_non_vision_keeps_block_and_warns() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    let img = image::RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 255]));
    img.save(cwd.join("pic.png")).unwrap();
    let read = ReadTool::new(
        fs(),
        cwd,
        ReadOpts {
            supports_images: false,
            ..ReadOpts::default()
        },
    );
    let r = read
        .execute(
            cid(),
            serde_json::json!({ "path": "pic.png" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    let text = first_text(&r);
    assert!(
        text.contains(
            "Current model does not support images. The image will be omitted from this request."
        ),
        "got: {text}"
    );
    // Verbatim Pi note prefix for the processed (output) mime.
    assert!(text.starts_with("Read image file ["), "got: {text}");
    // The image block is preserved even for non-vision models.
    assert!(r.content.iter().any(|c| matches!(c, Content::Image { .. })));
}

// A non-image file with an image extension must be read as TEXT now that detection is magic-byte
// based (mime.ts), not extension based.
#[tokio::test]
async fn read_fake_image_extension_is_text() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    std::fs::write(cwd.join("pic.png"), b"\x89PNG not-really a png\n").unwrap();
    let read = ReadTool::new(fs(), cwd, ReadOpts::default());
    let r = read
        .execute(
            cid(),
            serde_json::json!({ "path": "pic.png" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    assert!(!r.content.iter().any(|c| matches!(c, Content::Image { .. })));
    assert!(first_text(&r).contains("not-really"));
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
        .execute(
            cid(),
            serde_json::json!({ "path": "p.png" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    assert!(r.content.iter().any(|c| matches!(c, Content::Image { .. })));
}

// The `--no-default-features` arm of `ReadTool::read_image`. Pi has no build in which `read`
// withholds the image attachment (read.ts:247-263), so this arm is a KNOWN cyrup delta, recorded
// as such in `crates/cyrup-tools/Cargo.toml` (`default = ["inline-images"]`). What must not happen
// is that it degrades SILENTLY: the delta is only tolerable because the tool result says so in
// words the model can read. Until now the arm had zero tests — all three image tests here are
// `#[cfg(feature = "inline-images")]` — so nothing held it to that, and it was never even built by
// the gate. Run it with `cargo test -p cyrup-tools --no-default-features`.
#[cfg(not(feature = "inline-images"))]
#[tokio::test]
async fn read_image_without_inline_images_announces_the_delta_instead_of_going_quiet() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    // `image` is an unconditional dev-dependency, so the fixture is a real PNG on this arm too —
    // magic-byte detection (mime.ts) still has to classify it as an image.
    let img = image::RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 255]));
    img.save(cwd.join("p.png")).unwrap();
    let read = ReadTool::new(fs(), cwd, ReadOpts::default());
    let r = read
        .execute(
            cid(),
            serde_json::json!({ "path": "p.png" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();

    // No attachment on this arm — that is the delta.
    assert!(!r.content.iter().any(|c| matches!(c, Content::Image { .. })));
    let text = first_text(&r);
    // It was still recognised as an image, and the divergence is stated, not hidden.
    assert!(
        text.starts_with("Read image file [image/png]"),
        "got: {text}"
    );
    assert!(
        text.contains("[Image inlining is not enabled in this build (feature `inline-images`).]"),
        "the arm must self-announce or the delta is silent, got: {text}"
    );
}

// ---------------------------------------------------------------- A-03-3 edit

fn edit_tool(cwd: PathBuf) -> EditTool {
    EditTool::new(
        fs(),
        Arc::new(FileMutationLocks::new()),
        cwd,
        Default::default(),
    )
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
    assert_eq!(
        after, "\u{feff}one\r\nTWO\r\nthree\r\n",
        "CRLF+BOM preserved"
    );

    let details = r.details.unwrap();
    // Pi line-numbered display diff (`+NN TWO`).
    assert!(
        details["diff"].as_str().unwrap().contains("+2 TWO"),
        "diff: {}",
        details["diff"]
    );
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
    // Pi duplicate-match wording (edit-diff.ts:268-277).
    assert!(
        err.to_string().contains("Found 2 occurrences")
            && err.to_string().contains("must be unique"),
        "got: {}",
        err
    );
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
    assert_eq!(
        std::fs::read_to_string(cwd.join("a.txt")).unwrap(),
        "ALPHA\n"
    );

    // edits sent as a JSON string
    edit.execute(
        cid(),
        serde_json::json!({ "path": "b.txt", "edits": "[{\"oldText\":\"beta\",\"newText\":\"BETA\"}]" }),
        CancelToken::new(),
        noop_sink(),
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(cwd.join("b.txt")).unwrap(),
        "BETA\n"
    );
}

/// Replay of the agent preflight for one tool call, in Pi's order: `prepareArguments` then schema
/// validation then `execute` (`prepareToolCallArguments` → `validateToolArguments`,
/// agent-loop.ts:596-598,617-618; cyrup: `cyrup-agent/src/agent.rs` `prepare_arguments` →
/// `validate_tool_call`). A validation failure short-circuits to an isError tool result and the
/// tool never runs, which is what this returns as `Err`.
async fn preflight_execute(tool: &dyn Tool, raw: serde_json::Value) -> Result<ToolResult, String> {
    let prepared = tool.prepare_arguments(raw).await;
    let args = cyrup_provider::validate_tool_call(tool.parameters(), prepared)
        .map_err(|e| e.to_string())?;
    tool.execute(cid(), args, CancelToken::new(), noop_sink())
        .await
        .map_err(|e| e.to_string())
}

/// TOOL-002 — the legacy-argument shim must run BEFORE schema validation, or `{path, oldText,
/// newText}` / a stringified `edits` is rejected by `required:["path","edits"]` before `execute`
/// is ever reached. Pi attaches it as `prepareArguments: prepareEditArguments` (edit.ts:307).
#[tokio::test]
async fn edit_legacy_shim_survives_the_preflight_schema_gate() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    std::fs::write(cwd.join("a.txt"), "alpha\n").unwrap();
    std::fs::write(cwd.join("b.txt"), "beta\n").unwrap();
    let edit = edit_tool(cwd.clone());

    // Raw (un-prepared) legacy args are what the schema alone rejects — the reason the shim has to
    // sit on the `prepare_arguments` seam rather than inside `execute`.
    let legacy = serde_json::json!({ "path": "a.txt", "oldText": "alpha", "newText": "ALPHA" });
    assert!(
        cyrup_provider::validate_tool_call(edit.parameters(), legacy.clone()).is_err(),
        "edit's schema is expected to reject the legacy shape pre-normalization"
    );

    // Through the preflight the shim normalizes first, so the call succeeds and the file changes.
    let r = preflight_execute(&edit, legacy)
        .await
        .expect("legacy {oldText,newText} must edit");
    assert!(
        first_text(&r).contains("replaced 1 block"),
        "got: {}",
        first_text(&r)
    );
    assert_eq!(
        std::fs::read_to_string(cwd.join("a.txt")).unwrap(),
        "ALPHA\n"
    );

    // Same for `edits` sent as a JSON string (Pi: "Some models (Opus 4.6, GLM-5.1) send edits as a
    // JSON string instead of an array", edit.ts:100).
    let stringified = serde_json::json!({
        "path": "b.txt",
        "edits": "[{\"oldText\":\"beta\",\"newText\":\"BETA\"}]"
    });
    assert!(
        cyrup_provider::validate_tool_call(edit.parameters(), stringified.clone()).is_err(),
        "edit's schema is expected to reject a stringified `edits` pre-normalization"
    );
    preflight_execute(&edit, stringified)
        .await
        .expect("stringified `edits` must edit");
    assert_eq!(
        std::fs::read_to_string(cwd.join("b.txt")).unwrap(),
        "BETA\n"
    );

    // The normal shape is unaffected by the shim.
    std::fs::write(cwd.join("c.txt"), "gamma\n").unwrap();
    preflight_execute(
        &edit,
        serde_json::json!({
            "path": "c.txt",
            "edits": [{ "oldText": "gamma", "newText": "GAMMA" }]
        }),
    )
    .await
    .expect("canonical shape must still edit");
    assert_eq!(
        std::fs::read_to_string(cwd.join("c.txt")).unwrap(),
        "GAMMA\n"
    );
}

// ---------------------------------------------------------------- A-03-4 write

/// An `FsOps` that observes the GUARDED REGION directly: it counts live entrants to
/// `write_in_place` and records the high-water mark, the technique `lock.rs`'s
/// `same_path_serializes` already uses at the primitive level.
///
/// A first entrant waits (bounded) for a second to appear. With the mutation guard held, the
/// second is parked in `guard()` and never arrives, so the wait times out and `max_live` stays 1.
/// Remove the `guard()` call from `write.rs` and both enter, both observe `live == 2`, and the
/// assertion fails on EVERY run rather than on an unlucky interleaving — which is what TOOL-025's
/// residual asked for. The property is pi's `withFileMutationQueue` (write.ts:208): mutual
/// exclusion per resolved path, not a surviving byte pattern.
struct MutexProbeFs {
    inner: LocalFs,
    live: Arc<AtomicUsize>,
    max_live: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl FsOps for MutexProbeFs {
    async fn read(&self, path: &Path) -> Result<Vec<u8>, ToolError> {
        self.inner.read(path).await
    }
    async fn write_in_place(&self, path: &Path, bytes: &[u8]) -> Result<(), ToolError> {
        let now = self.live.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_live.fetch_max(now, Ordering::SeqCst);
        // Give a would-be second entrant every chance to arrive and be counted.
        for _ in 0..50 {
            if self.live.load(Ordering::SeqCst) > 1 {
                self.max_live
                    .fetch_max(self.live.load(Ordering::SeqCst), Ordering::SeqCst);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        let r = self.inner.write_in_place(path, bytes).await;
        self.live.fetch_sub(1, Ordering::SeqCst);
        r
    }
    async fn access(&self, path: &Path, mode: crate::ops::Access) -> Result<(), ToolError> {
        self.inner.access(path, mode).await
    }
    async fn metadata(&self, path: &Path) -> Result<crate::ops::Meta, ToolError> {
        self.inner.metadata(path).await
    }
    async fn read_dir(&self, path: &Path) -> Result<Vec<crate::ops::DirEntry>, ToolError> {
        self.inner.read_dir(path).await
    }
    fn walk(
        &self,
        root: &Path,
        opts: crate::ops::WalkOpts,
    ) -> cyrup_core::EventStream<Result<crate::ops::WalkItem, ToolError>> {
        self.inner.walk(root, opts)
    }
}

/// TOOL-025 residual: renamed from `write_creates_dirs_and_serializes`, which named an ordering it
/// never observed. What it observes is (a) parent-directory creation and (b) mutual exclusion —
/// now asserted STRUCTURALLY via `MutexProbeFs` rather than by hoping a corrupting interleaving
/// happens to occur, plus the surviving-payload disjunction kept as a second, weaker witness.
#[tokio::test]
async fn write_creates_dirs_and_holds_one_mutator_per_path() {
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
    assert!(first_text(&r).contains("Successfully wrote 5 bytes"));
    assert_eq!(
        std::fs::read_to_string(cwd.join("nested/deep/f.txt")).unwrap(),
        "hello"
    );

    // Concurrent writes to the same path: no corruption (final == one full content).
    //
    // The two payloads have DIFFERENT LENGTHS on purpose. They used to be `"AAAA"`/`"BBBB"`, which
    // made this half vacuous: two same-length writes can interleave arbitrarily and still land on a
    // 4-byte file, and the temp-file+rename backend made the outcome atomic for free anyway. The
    // backend is now an in-place `O_TRUNC` write (TOOL-004, matching pi's `fsWriteFile`), so the
    // whole exclusion property rests on [`FileMutationLocks`] alone — pi's `withFileMutationQueue`
    // (write.ts:9) plays the identical role. With unequal lengths any interleaving is observable:
    // a lost lock leaves a short read (`"AAAA"` where `B` was last), a tail (`"AAAABBBB…"`), or a
    // truncated prefix — none of which match either payload exactly.
    let live = Arc::new(AtomicUsize::new(0));
    let max_live = Arc::new(AtomicUsize::new(0));
    let probe = Arc::new(MutexProbeFs {
        inner: LocalFs,
        live: Arc::clone(&live),
        max_live: Arc::clone(&max_live),
    });
    let w = Arc::new(WriteTool::new(
        probe,
        locks,
        cwd.clone(),
        Default::default(),
    ));
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
                serde_json::json!({ "path": "race.txt", "content": "BBBBBBBBBBBBBBBB" }),
                CancelToken::new(),
                noop_sink(),
            )
            .await
        })
    };
    a.await.unwrap().unwrap();
    b.await.unwrap().unwrap();
    // The load-bearing assertion: at most one mutator was ever inside the guarded region. This is
    // the property `withFileMutationQueue` provides (write.ts:208) and it fails deterministically
    // if the `guard()` call is removed from `write.rs`.
    assert_eq!(
        max_live.load(Ordering::SeqCst),
        1,
        "two mutators were inside `write_in_place` for the same path at once"
    );
    assert_eq!(
        live.load(Ordering::SeqCst),
        0,
        "every entrant left the guarded region"
    );
    // Weaker second witness, kept: the surviving bytes match exactly one payload. The two payloads
    // have DIFFERENT LENGTHS on purpose — with equal lengths any interleaving still lands on a
    // same-sized file and the check is vacuous. The backend is an in-place `O_TRUNC` write
    // (TOOL-004, matching pi's `fsWriteFile`), so a lost lock leaves a short read, a tail, or a
    // truncated prefix, none of which match either payload.
    let final_content = std::fs::read_to_string(cwd.join("race.txt")).unwrap();
    assert!(
        final_content == "AAAA" || final_content == "BBBBBBBBBBBBBBBB",
        "the two concurrent writes interleaved; got: {final_content:?}"
    );
}

// ---------------------------------------------------------------- A-03-5 bash

fn bash_tool(cwd: PathBuf, opts: BashOpts) -> ShellTool {
    ShellTool::bash(proc(), cwd, opts)
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
        BashOpts {
            max_lines: 5,
            max_bytes: 100,
            ..Default::default()
        },
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
    let path = details["fullOutputPath"]
        .as_str()
        .expect("full output path");
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
    // A real git repo: Pi's grep is plain ripgrep (require_git=true default), so `.gitignore` is
    // honored only *inside* a repo. The `ignore` crate detects the repo by this `.git` dir.
    std::fs::create_dir(p.join(".git")).unwrap();
    dir
}

// grep parity: Pi's grep passes NO `--no-require-git` (grep.ts:215-219), so OUTSIDE any git repo a
// stray `.gitignore` is NOT applied (ripgrep default require_git=true) — the opposite of Pi's `find`
// (fd `--no-require-git`, which honors it outside a repo). A gitignored file must still be searched.
#[tokio::test]
async fn grep_gitignore_not_applied_outside_repo() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join(".gitignore"), "*.log\n").unwrap();
    std::fs::write(root.join("keep.txt"), "needle here\n").unwrap();
    std::fs::write(root.join("skip.log"), "needle here\n").unwrap();

    let grep = GrepTool::new(fs(), root.to_path_buf(), GrepOpts::default());
    let r = grep
        .execute(
            cid(),
            serde_json::json!({ "pattern": "needle" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    let text = first_text(&r);
    // No `.git` here → gitignore inert → the gitignored `skip.log` IS searched (matches Pi grep).
    assert!(text.contains("keep.txt:1: needle here"), "got: {text}");
    assert!(
        text.contains("skip.log:1: needle here"),
        "gitignore wrongly applied outside repo: {text}"
    );
}

#[tokio::test]
async fn grep_format_and_gitignore_and_no_matches() {
    let dir = fixture_repo();
    let cwd = dir.path().to_path_buf();
    let grep = GrepTool::new(fs(), cwd.clone(), GrepOpts::default());

    let r = grep
        .execute(
            cid(),
            serde_json::json!({ "pattern": "hello" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    let text = first_text(&r);
    // documented shape: filepath:line: match
    assert!(text.contains("a.txt:1: hello world"), "got: {text}");
    assert!(text.contains("sub/c.txt:1: hello nested"), "got: {text}");
    // gitignored *.log excluded
    assert!(!text.contains("b.log"), "gitignore not respected: {text}");

    let none = grep
        .execute(
            cid(),
            serde_json::json!({ "pattern": "zzz_nomatch" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    assert_eq!(first_text(&none), "No matches found");
}

/// TOOL-005 — Pi runs ripgrep with no `--text`, so traversed binary files are cut off at the first
/// NUL (`BinaryDetection::quit`) and contribute no match lines. Raw bytes must never reach the
/// model-facing result.
#[tokio::test]
async fn grep_skips_binary_files() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    std::fs::write(cwd.join("blob.bin"), b"hello\x00\xff\xfe world hello").unwrap();

    let grep = GrepTool::new(fs(), cwd.clone(), GrepOpts::default());
    let r = grep
        .execute(
            cid(),
            serde_json::json!({ "pattern": "hello" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    assert_eq!(
        first_text(&r),
        "No matches found",
        "binary file must contribute no matches"
    );

    // A NUL after a match line still suppresses that file, and a plain text file alongside it is
    // unaffected — binary detection must not turn grep into a no-op.
    std::fs::write(
        cwd.join("later.bin"),
        b"hello text line\nmore\n\x00\x01\x02binary\n",
    )
    .unwrap();
    std::fs::write(cwd.join("plain.txt"), "hello plain\n").unwrap();
    let r = grep
        .execute(
            cid(),
            serde_json::json!({ "pattern": "hello" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    let text = first_text(&r);
    assert!(
        text.contains("plain.txt:1: hello plain"),
        "text file must still match: {text}"
    );
    assert!(
        !text.contains('\u{fffd}'),
        "no lossy-decoded bytes may reach the result: {text}"
    );
    assert!(!text.contains("blob.bin"), "got: {text}");
    assert!(!text.contains("later.bin"), "got: {text}");
}

#[tokio::test]
async fn grep_and_find_search_hidden_files() {
    // Pi runs `rg --hidden` / `fd --hidden` (grep.ts:215, find.ts:224): dotfiles ARE searched while
    // `.gitignore` is still honored.
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    std::fs::write(cwd.join(".env"), "SECRET=hello-hidden\n").unwrap();
    std::fs::create_dir(cwd.join(".config")).unwrap();
    std::fs::write(cwd.join(".config/app.toml"), "key = hidden\n").unwrap();
    std::fs::write(cwd.join(".gitignore"), "ignored.txt\n").unwrap();
    std::fs::write(cwd.join("ignored.txt"), "hello-hidden\n").unwrap();
    // Inside a real repo, so grep (ripgrep default require_git=true) honors `.gitignore` while
    // `--hidden` still searches dotfiles — the exact combination this test asserts.
    std::fs::create_dir(cwd.join(".git")).unwrap();

    let grep = GrepTool::new(fs(), cwd.clone(), GrepOpts::default());
    let r = grep
        .execute(
            cid(),
            serde_json::json!({ "pattern": "hello-hidden" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    let text = first_text(&r);
    assert!(
        text.contains(".env:1:"),
        "hidden dotfile must be searched: {text}"
    );
    // gitignored file is still excluded even though hidden search is on.
    assert!(
        !text.contains("ignored.txt"),
        "gitignore must still apply: {text}"
    );

    let find = FindTool::new(fs(), cwd, FindOpts::default());
    let r = find
        .execute(
            cid(),
            serde_json::json!({ "pattern": "*.toml" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    assert!(
        first_text(&r).contains(".config/app.toml"),
        "got: {}",
        first_text(&r)
    );
}

#[tokio::test]
async fn edit_fuzzy_matches_curly_quote_end_to_end() {
    // The model sends an ASCII apostrophe but disk has a curly U+2019; Pi's fuzzy fallback matches.
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    std::fs::write(cwd.join("s.rs"), "let s = \u{2019}hi\u{2019};\n").unwrap();
    let edit = edit_tool(cwd.clone());
    let r = edit
        .execute(
            cid(),
            serde_json::json!({
                "path": "s.rs",
                "edits": [{ "oldText": "let s = 'hi';", "newText": "let s = 'bye';" }]
            }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    assert!(first_text(&r).contains("replaced 1 block"));
    assert_eq!(
        std::fs::read_to_string(cwd.join("s.rs")).unwrap(),
        "let s = 'bye';\n"
    );
}

#[tokio::test]
async fn find_format_and_gitignore_and_sentinel() {
    let dir = fixture_repo();
    let cwd = dir.path().to_path_buf();
    let find = FindTool::new(fs(), cwd.clone(), FindOpts::default());

    let r = find
        .execute(
            cid(),
            serde_json::json!({ "pattern": "*.txt" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    let text = first_text(&r);
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines.contains(&"a.txt"), "got: {text}");
    assert!(lines.contains(&"sub/c.txt"), "got: {text}");

    // gitignored log not found
    let none = find
        .execute(
            cid(),
            serde_json::json!({ "pattern": "*.log" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    assert_eq!(first_text(&none), "No files found matching pattern");

    // directory suffix '/'
    let dirs = find
        .execute(
            cid(),
            serde_json::json!({ "pattern": "sub" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    assert!(
        first_text(&dirs).contains("sub/"),
        "got: {}",
        first_text(&dirs)
    );
}

// G2 — the find git-boundary fix (find.ts:226-240, issue #5960). Inside a git repo, fd's default
// git-aware behavior lets a parent `.gitignore` rule stop at a NESTED repo boundary. Cyrup detects
// the enclosing repo and sets `require_git(true)` accordingly. Observable: the outer repo's
// `*.log` rule ignores `outer.log` but does NOT cross into the nested repo, so `nested/inner.log`
// IS discovered.
#[tokio::test]
async fn find_git_boundary_stops_at_nested_repo() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join(".gitignore"), "*.log\n").unwrap();
    std::fs::write(root.join("outer.log"), "x").unwrap();
    std::fs::create_dir_all(root.join("nested/.git")).unwrap();
    std::fs::write(root.join("nested/inner.log"), "x").unwrap();

    let find = FindTool::new(fs(), root.to_path_buf(), FindOpts::default());
    let r = find
        .execute(
            cid(),
            serde_json::json!({ "pattern": "*.log" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    let text = first_text(&r);
    let lines: Vec<&str> = text.lines().collect();
    // Parent `.gitignore` stops at the nested repo boundary — inner.log surfaces.
    assert!(
        lines.contains(&"nested/inner.log"),
        "nested repo boundary not honored: {text}"
    );
    // The outer repo's own file remains gitignored.
    assert!(
        !lines.contains(&"outer.log"),
        "outer .gitignore should apply in its own repo: {text}"
    );
}

// G2 — OUTSIDE any git repo, fd passes `--no-require-git` so `.gitignore` is STILL honored. Cyrup
// detects no enclosing `.git` and sets `require_git(false)`. A temp dir has no `.git`, so the
// gitignored `*.log` file must not appear.
#[tokio::test]
async fn find_gitignore_honored_outside_repo() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join(".gitignore"), "*.log\n").unwrap();
    std::fs::write(root.join("keep.txt"), "x").unwrap();
    std::fs::write(root.join("skip.log"), "x").unwrap();

    let find = FindTool::new(fs(), root.to_path_buf(), FindOpts::default());
    let logs = find
        .execute(
            cid(),
            serde_json::json!({ "pattern": "*.log" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    assert_eq!(first_text(&logs), "No files found matching pattern");
    let txt = find
        .execute(
            cid(),
            serde_json::json!({ "pattern": "*.txt" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    assert!(
        first_text(&txt).lines().any(|l| l == "keep.txt"),
        "got: {}",
        first_text(&txt)
    );
}

// G1 — `timeout` is Pi's `Type.Number` (float SECONDS, bash.ts:42). `timeout:2.5` must deserialize
// and resolve to 2500ms. Run a 30s sleep with a 2.5s timeout and assert it is killed at ~2.5s and
// the message prints the original `2.5` value (bash.ts:414-415).
#[tokio::test]
async fn bash_timeout_fractional_seconds() {
    let dir = tempfile::tempdir().unwrap();
    let bash = bash_tool(dir.path().to_path_buf(), BashOpts::default());
    let start = std::time::Instant::now();
    let err = bash
        .execute(
            cid(),
            serde_json::json!({ "command": "sleep 30", "timeout": 2.5 }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap_err();
    let elapsed = start.elapsed();
    let msg = err.to_string();
    assert!(
        msg.contains("Command timed out after 2.5 seconds"),
        "got: {msg}"
    );
    // TOOL-026: the LOWER bound is the load-bearing half — it proves the 2.5s value was honoured
    // rather than some default. The upper bound only has to stay far below the 30s sleep to prove
    // the timeout fired at all; the old 4s ceiling left ~1.5s for scheduling plus the
    // SIGTERM→grace→SIGKILL escalation plus reaping under an arbitrarily loaded run, which is a
    // wall-clock figure this test cannot control.
    assert!(
        elapsed >= std::time::Duration::from_millis(2300)
            && elapsed < std::time::Duration::from_secs(15),
        "expected ~2.5s kill (and, at minimum, well before the 30s sleep), got {elapsed:?}"
    );
}

// G1 — `resolveTimeoutMs` rejects non-positive values with Pi's EXACT error string, before running
// anything (bash.ts:29-31 → re-thrown verbatim via `throw err`, bash.ts:417).
#[tokio::test]
async fn bash_timeout_zero_is_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let bash = bash_tool(dir.path().to_path_buf(), BashOpts::default());
    let err = bash
        .execute(
            cid(),
            // A command that would succeed instantly if it ever ran — proves the guard fires first.
            serde_json::json!({ "command": "echo should-not-run", "timeout": 0 }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Invalid timeout: must be a finite number of seconds"
    );
}

// G1 — a negative timeout is likewise rejected with the same error (bash.ts:29 `timeout <= 0`).
#[tokio::test]
async fn bash_timeout_negative_is_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let bash = bash_tool(dir.path().to_path_buf(), BashOpts::default());
    let err = bash
        .execute(
            cid(),
            serde_json::json!({ "command": "echo x", "timeout": -1.5 }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Invalid timeout: must be a finite number of seconds"
    );
}

// G1 — a timeout whose millisecond form exceeds the 32-bit ceiling is rejected with Pi's exact
// maximum message (bash.ts:34-35, `${MAX_TIMEOUT_MS / 1000}` = 2147483.647).
#[tokio::test]
async fn bash_timeout_over_maximum_is_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let bash = bash_tool(dir.path().to_path_buf(), BashOpts::default());
    let err = bash
        .execute(
            cid(),
            // 2_147_484 s * 1000 = 2_147_484_000 ms > 2_147_483_647.
            serde_json::json!({ "command": "echo x", "timeout": 2_147_484 }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Invalid timeout: maximum is 2147483.647 seconds"
    );
}

// G1 — the largest valid timeout (exactly 2147483.647s → 2_147_483_647ms) is accepted. It must NOT
// error at resolve time; the command runs to completion well within the window.
#[tokio::test]
async fn bash_timeout_at_maximum_is_valid() {
    let dir = tempfile::tempdir().unwrap();
    let bash = bash_tool(dir.path().to_path_buf(), BashOpts::default());
    let r = bash
        .execute(
            cid(),
            serde_json::json!({ "command": "echo ok", "timeout": 2_147_483.647 }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    assert!(first_text(&r).contains("ok"));
}

#[tokio::test]
async fn find_limit_truncation() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    for i in 0..10 {
        std::fs::write(cwd.join(format!("f{i}.txt")), "x").unwrap();
    }
    let find = FindTool::new(
        fs(),
        cwd,
        FindOpts {
            limit: 2,
            max_bytes: 50 * 1024,
        },
    );
    let r = find
        .execute(
            cid(),
            serde_json::json!({ "pattern": "*.txt" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    assert!(
        first_text(&r).contains("results limit reached"),
        "got: {}",
        first_text(&r)
    );
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
        .execute(
            cid(),
            serde_json::json!({}),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    let text = first_text(&r);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines,
        vec![".dot", "a.txt", "B.txt", "zdir/"],
        "case-insensitive sort + '/'"
    );

    // empty dir
    let empty = tempfile::tempdir().unwrap();
    let ls2 = LsTool::new(fs(), empty.path().to_path_buf(), LsOpts::default());
    let r = ls2
        .execute(
            cid(),
            serde_json::json!({}),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    assert_eq!(first_text(&r), "(empty directory)");

    // not a directory
    let err = ls
        .execute(
            cid(),
            serde_json::json!({ "path": "a.txt" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("Not a directory"));

    // gap #8 — missing path uses Pi's exact literal `Path not found: ${dirPath}` (ls.ts:129),
    // not the prior `Directory not found:`.
    let err = ls
        .execute(
            cid(),
            serde_json::json!({ "path": "nope" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().starts_with("Path not found:"), "got: {err}");
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
    assert_eq!(reg.all().len(), 8);

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
            ..Default::default()
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

// A `ProcOps` that emits two chunks within the throttle window, then HOLDS the stream open past the
// 100ms throttle (so the trailing-edge timer can fire mid-stream) before emitting a final chunk.
// Mirrors Pi's `scheduleOutputUpdate` leading+trailing cadence (bash.ts:302-336).
struct ScriptedProc;
#[async_trait::async_trait]
impl ProcOps for ScriptedProc {
    async fn exec(
        &self,
        _spec: ExecSpec,
        _cancel: CancelToken,
        _timeout: Option<std::time::Duration>,
        on_data: &mut (dyn for<'a> FnMut(&'a [u8]) + Send),
    ) -> Result<ExitStatus, ToolError> {
        // First chunk -> leading-edge emit ("a"). Second chunk arrives within the throttle window,
        // so it can only reach the consumer via the scheduled TRAILING flush.
        on_data(b"a");
        on_data(b"b");
        // Keep the stream (and thus the channel) open past the throttle; with paused time the
        // flusher's 100ms trailing timer fires here -> mid-stream "ab" flush.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        // After the trailing flush, this chunk is again past the throttle -> leading-edge "abc".
        on_data(b"c");
        Ok(ExitStatus::Exited(0))
    }
}

fn update_text(u: &ToolUpdate) -> String {
    u.content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.to_string()),
            _ => None,
        })
        .collect()
}

// gap — Pi flushes a sub-threshold output burst MID-STREAM via a scheduled trailing-edge timer
// (`scheduleOutputUpdate` -> `setTimeout` -> `emitOutputUpdate`, bash.ts:302-336). cyrup must too:
// the second chunk ("b") arrives inside the throttle window after the leading "a" emit, so the only
// way the consumer sees "ab" BEFORE the final settle is the trailing flush. `start_paused` makes the
// timer deterministic (tokio auto-advances virtual time to the next deadline when otherwise idle).
#[tokio::test(start_paused = true)]
async fn bash_trailing_edge_flush_emits_midstream() {
    let updates: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink: ToolUpdateSink = {
        let updates = updates.clone();
        Box::new(move |u: ToolUpdate| {
            updates.lock().unwrap().push(update_text(&u));
        })
    };
    let dir = tempfile::tempdir().unwrap();
    let bash = ShellTool::bash(
        Arc::new(ScriptedProc),
        dir.path().to_path_buf(),
        BashOpts::default(),
    );
    let r = bash
        .execute(
            cid(),
            serde_json::json!({ "command": "x" }),
            CancelToken::new(),
            sink,
        )
        .await
        .unwrap();

    let recorded = updates.lock().unwrap().clone();
    // Leading edge delivered "a".
    assert!(
        recorded.iter().any(|t| t == "a"),
        "missing leading-edge 'a': {recorded:?}"
    );
    // The TRAILING-edge timer flushed the coalesced "ab" mid-stream (the regression under test):
    // without it, "ab" never appears — the consumer would jump straight from "a" to "abc".
    assert!(
        recorded.iter().any(|t| t == "ab"),
        "missing mid-stream trailing flush 'ab': {recorded:?}"
    );
    // Final settled content includes all output.
    assert!(
        first_text(&r).contains("abc"),
        "final result missing 'abc': {}",
        first_text(&r)
    );
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
    reg.insert(Arc::new(EchoRead {
        params: serde_json::json!({ "type": "object" }),
    }));
    assert_eq!(reg.all().len(), 8, "override does not add a new slot");
    let read = reg.get("read").unwrap();
    let r = read
        .execute(
            cid(),
            serde_json::json!({}),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    assert_eq!(first_text(&r), "overridden read");

    // Throwing tool -> Err (mapped to isError by the runtime).
    reg.insert(Arc::new(Boom));
    let boom = reg.get("boom").unwrap();
    let err = boom
        .execute(
            cid(),
            serde_json::json!({}),
            CancelToken::new(),
            noop_sink(),
        )
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
    async fn write_in_place(&self, path: &Path, bytes: &[u8]) -> Result<(), ToolError> {
        self.inner.write_in_place(path, bytes).await
    }
    async fn access(&self, path: &Path, mode: crate::ops::Access) -> Result<(), ToolError> {
        self.inner.access(path, mode).await
    }
    async fn metadata(&self, path: &Path) -> Result<crate::ops::Meta, ToolError> {
        self.inner.metadata(path).await
    }
    async fn read_dir(&self, path: &Path) -> Result<Vec<crate::ops::DirEntry>, ToolError> {
        self.inner.read_dir(path).await
    }
    fn walk(
        &self,
        root: &Path,
        opts: crate::ops::WalkOpts,
    ) -> cyrup_core::EventStream<Result<crate::ops::WalkItem, ToolError>> {
        self.inner.walk(root, opts)
    }
}

#[tokio::test]
async fn tool_logic_is_backend_agnostic() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    std::fs::write(cwd.join("f.txt"), "data\n").unwrap();
    let reads = Arc::new(AtomicUsize::new(0));
    let counting: Arc<dyn FsOps> = Arc::new(CountingFs {
        inner: fs(),
        reads: reads.clone(),
    });
    let read = ReadTool::new(counting, cwd, ReadOpts::default());
    let r = read
        .execute(
            cid(),
            serde_json::json!({ "path": "f.txt" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    assert!(first_text(&r).contains("data"));
    assert!(
        reads.load(Ordering::SeqCst) >= 1,
        "tool routed through the seam"
    );
}

// ---------------------------------------------------------------- round-2 1:1 additions

// gap #3 — spawnHook lets an extension rewrite {command,cwd,env} before exec (bash.ts:139-144).
#[tokio::test]
async fn bash_spawn_hook_rewrites_command() {
    use crate::config::BashSpawnContext;
    let dir = tempfile::tempdir().unwrap();
    let hook: crate::config::BashSpawnHook = Arc::new(|mut ctx: BashSpawnContext| {
        // Replace the model's command entirely.
        ctx.command = "echo rewritten-by-hook".to_string();
        ctx
    });
    let bash = bash_tool(
        dir.path().to_path_buf(),
        BashOpts {
            spawn_hook: Some(hook),
            ..Default::default()
        },
    );
    let r = bash
        .execute(
            cid(),
            serde_json::json!({ "command": "echo original" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    let text = first_text(&r);
    assert!(text.contains("rewritten-by-hook"), "got: {text}");
    assert!(!text.contains("original"), "got: {text}");
}

// gap #5 — bin_dir is prepended to the child PATH (getShellEnv, shell.ts:122-134).
#[tokio::test]
async fn bash_bin_dir_prepended_to_path() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("managed-bin");
    std::fs::create_dir_all(&bin).unwrap();
    let bash = bash_tool(
        dir.path().to_path_buf(),
        BashOpts {
            bin_dir: Some(bin.clone()),
            ..Default::default()
        },
    );
    let r = bash
        .execute(
            cid(),
            serde_json::json!({ "command": "printf '%s' \"$PATH\"" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    let text = first_text(&r);
    assert!(
        text.starts_with(&bin.to_string_lossy().into_owned()),
        "PATH was: {text}"
    );
}

// gap #4 — an explicit but missing shellPath yields the `Custom shell path not found` error,
// surfaced per-exec (shell.ts:73; bash.ts:69).
#[tokio::test]
async fn bash_missing_shell_path_errors() {
    let dir = tempfile::tempdir().unwrap();
    let bash = bash_tool(
        dir.path().to_path_buf(),
        BashOpts {
            shell_path: Some("/no/such/shell".to_string()),
            ..Default::default()
        },
    );
    let err = bash
        .execute(
            cid(),
            serde_json::json!({ "command": "echo hi" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("Custom shell path not found"),
        "got: {err}"
    );
}

// Pi always emits its initial empty `onUpdate({content:[],details:undefined})` (bash.ts:355-357)
// strictly BEFORE `ops.exec` runs `resolveTimeoutMs`/the abort check/`getShellConfig`
// (bash.ts:85-89), so even a hard-failing shell resolution must be preceded by that update.
#[tokio::test]
async fn bash_missing_shell_path_still_emits_initial_empty_update_first() {
    let dir = tempfile::tempdir().unwrap();
    let updates: Arc<std::sync::Mutex<Vec<ToolUpdate>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink: ToolUpdateSink = {
        let updates = updates.clone();
        Box::new(move |u| {
            updates.lock().unwrap().push(u);
        })
    };
    let bash = bash_tool(
        dir.path().to_path_buf(),
        BashOpts {
            shell_path: Some("/no/such/shell".to_string()),
            ..Default::default()
        },
    );
    let err = bash
        .execute(
            cid(),
            serde_json::json!({ "command": "echo hi" }),
            CancelToken::new(),
            sink,
        )
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("Custom shell path not found"),
        "got: {err}"
    );
    let seen = updates.lock().unwrap();
    assert_eq!(
        seen.len(),
        1,
        "expected exactly one (the initial empty) update, got {seen:?}"
    );
    assert!(seen[0].content.is_empty());
    assert!(seen[0].details.is_none());
}

// L4 round-17 finding #2a: Pi's abort check (`if (signal?.aborted) throw new Error("aborted")`,
// bash.ts:86-88) sits strictly BETWEEN `resolveTimeoutMs` and `getShellConfig` (bash.ts:85,89), so
// an ALREADY-cancelled run must surface "Command aborted" even when the configured `shellPath` is
// also invalid — `getShellConfig` (and its `Custom shell path not found` error) is never reached at
// all once the abort check has already thrown. This is the inverse of
// `bash_missing_shell_path_errors` above (same invalid `shellPath`, but a live, non-cancelled
// token) — proving the abort check genuinely runs BEFORE shell resolution, not after.
#[tokio::test]
async fn bash_pre_cancelled_reports_aborted_even_with_an_invalid_shell_path() {
    let dir = tempfile::tempdir().unwrap();
    let bash = bash_tool(
        dir.path().to_path_buf(),
        BashOpts {
            shell_path: Some("/no/such/shell".to_string()),
            ..Default::default()
        },
    );
    let cancel = CancelToken::new();
    cancel.cancel();
    let err = bash
        .execute(
            cid(),
            serde_json::json!({ "command": "echo hi" }),
            cancel,
            noop_sink(),
        )
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("aborted"), "got: {msg}");
    assert!(
        !msg.contains("Custom shell path not found"),
        "the abort check must short-circuit before shellPath is ever resolved, got: {msg}"
    );
}

// gap #13 — read offset bound is `allLines.length` (the trailing-newline phantom line counts),
// and the out-of-bounds error reads `(N lines total)` (read.ts:268-275).
#[tokio::test]
async fn read_offset_bound_counts_trailing_newline_line() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    // 3 newline-terminated lines ⇒ split("\n") has 4 elements (the 4th empty).
    std::fs::write(cwd.join("f.txt"), "a\nb\nc\n").unwrap();
    let read = ReadTool::new(fs(), cwd, ReadOpts::default());
    // offset=4 selects the empty phantom line (in-bounds), returns "".
    let ok = read
        .execute(
            cid(),
            serde_json::json!({ "path": "f.txt", "offset": 4 }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    assert_eq!(first_text(&ok), "");
    // offset=5 is past the 4-element basis ⇒ error worded with "lines total".
    let err = read
        .execute(
            cid(),
            serde_json::json!({ "path": "f.txt", "offset": 5 }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("beyond end of file (4 lines total)"),
        "got: {err}"
    );
}

// gap #8 — write reports JS string length (UTF-16 units), not UTF-8 bytes (write.ts:222).
#[tokio::test]
async fn write_reports_utf16_length() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    let write = WriteTool::new(
        fs(),
        Arc::new(FileMutationLocks::new()),
        cwd.clone(),
        Default::default(),
    );
    // "é𝄞" = 1 (é, 1 UTF-16 unit) + 1 (𝄞, astral, 2 UTF-16 units) = 3 UTF-16 units, 6 UTF-8 bytes.
    let r = write
        .execute(
            cid(),
            serde_json::json!({ "path": "u.txt", "content": "é𝄞" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    assert!(
        first_text(&r).contains("Successfully wrote 3 bytes to u.txt"),
        "got: {}",
        first_text(&r)
    );
    // gap #6 — Pi declares `ToolDefinition<…, undefined>` and returns `details: undefined`
    // (write.ts:223); cyrup must emit `None`, never a `{bytesWritten}` payload.
    assert!(
        r.details.is_none(),
        "write must emit no details: {:?}",
        r.details
    );
}

// gap #11 — a small directory with no truncation and no limit-hit emits NO `details` object (ls.ts).
#[tokio::test]
async fn ls_omits_details_when_not_notable() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    std::fs::write(cwd.join("only.txt"), "x").unwrap();
    let ls = LsTool::new(fs(), cwd, LsOpts::default());
    let r = ls
        .execute(
            cid(),
            serde_json::json!({}),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    assert!(
        r.details.is_none(),
        "details should be omitted: {:?}",
        r.details
    );
}

// gap #1 — magic-byte detection for each supported type, animated-PNG / lossless-JPEG rejection.
#[test]
fn image_magic_detection() {
    use crate::ops::ImageMime;
    let png_sig = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
    // Valid PNG: IHDR length 13 + "IHDR".
    let mut png = Vec::from(png_sig);
    png.extend_from_slice(&[0, 0, 0, 13]); // length BE
    png.extend_from_slice(b"IHDR");
    png.extend_from_slice(&[0u8; 8]);
    assert_eq!(ImageMime::from_magic(&png), Some(ImageMime::Png));
    // Animated PNG (acTL before IDAT) is rejected.
    let mut apng = Vec::from(png_sig);
    apng.extend_from_slice(&[0, 0, 0, 13]);
    apng.extend_from_slice(b"IHDR");
    apng.extend_from_slice(&[0u8; 13 + 4]); // IHDR data + crc
    apng.extend_from_slice(&[0, 0, 0, 8]); // acTL length
    apng.extend_from_slice(b"acTL");
    assert_eq!(ImageMime::from_magic(&apng), None);
    // JPEG, and the lossless 0xF7 variant rejected.
    assert_eq!(
        ImageMime::from_magic(&[0xff, 0xd8, 0xff, 0xe0]),
        Some(ImageMime::Jpeg)
    );
    assert_eq!(ImageMime::from_magic(&[0xff, 0xd8, 0xff, 0xf7]), None);
    assert_eq!(ImageMime::from_magic(b"GIF89a"), Some(ImageMime::Gif));
    let mut webp = Vec::from(*b"RIFF");
    webp.extend_from_slice(&[0, 0, 0, 0]);
    webp.extend_from_slice(b"WEBP");
    assert_eq!(ImageMime::from_magic(&webp), Some(ImageMime::Webp));
    // Plain text is not an image.
    assert_eq!(ImageMime::from_magic(b"hello world\n"), None);
}

/// TOOL-035 — Pi sniffs a fixed `IMAGE_TYPE_SNIFF_BYTES = 4100`-byte header (mime.ts:3, read at
/// `:28-30`), so `isAnimatedPng`'s chunk walk bails at mime.ts:51 the moment it steps past the
/// buffer and an `acTL` beyond the window is INVISIBLE — the file is reported as `image/png`.
/// cyrup handed the whole file to the sniffer, found the far `acTL`, returned `None`, and dropped
/// the file into `read`'s TEXT branch, so the model got `from_utf8_lossy` of a PNG.
///
/// RED before the fix (`from_file_head` did not exist and `from_magic` saw the whole buffer, so
/// the far-`acTL` case returned `None`); GREEN after.
#[test]
fn image_sniff_window_matches_pis_4100_bytes() {
    use crate::ops::{IMAGE_TYPE_SNIFF_BYTES, ImageMime};
    assert_eq!(
        IMAGE_TYPE_SNIFF_BYTES, 4100,
        "mime.ts:3 `IMAGE_TYPE_SNIFF_BYTES = 4100`"
    );

    let png_sig = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
    // signature(8) + IHDR(4 len + 4 type + 13 data + 4 crc = 25) = 33 bytes so far.
    let mut base = Vec::from(png_sig);
    base.extend_from_slice(&[0, 0, 0, 13]);
    base.extend_from_slice(b"IHDR");
    base.extend_from_slice(&[0u8; 13 + 4]);

    // A large ancillary `iTXt` chunk that pushes the following `acTL` past byte 4100.
    let filler_len: usize = 5000;
    let mut far = base.clone();
    far.extend_from_slice(&u32::try_from(filler_len).unwrap().to_be_bytes());
    far.extend_from_slice(b"iTXt");
    far.extend_from_slice(&vec![0u8; filler_len]);
    far.extend_from_slice(&[0u8; 4]); // crc
    far.extend_from_slice(&[0, 0, 0, 8]);
    far.extend_from_slice(b"acTL");
    // Pi's window never reaches that chunk, so the verdict is `image/png`.
    assert_eq!(ImageMime::from_file_head(&far), Some(ImageMime::Png));

    // The animation check is NOT disabled: an `acTL` INSIDE the window still rejects, exactly as
    // `image_magic_detection` above pins for the unbounded predicate.
    let mut near = base.clone();
    near.extend_from_slice(&[0, 0, 0, 8]);
    near.extend_from_slice(b"acTL");
    assert_eq!(ImageMime::from_file_head(&near), None);
}

// gap #1 — an oversized image is resized to fit 2000px and carries the coordinate-mapping
// dimension note (image-resize-core.ts + formatDimensionNote). Exercises decode→orientation→
// resize→encode end to end.
#[cfg(feature = "inline-images")]
#[tokio::test]
async fn read_image_oversized_is_resized_with_dimension_note() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    // 2400x10 PNG ⇒ wider than the 2000px cap ⇒ must be resized.
    let img = image::RgbaImage::from_pixel(2400, 10, image::Rgba([1, 2, 3, 255]));
    img.save(cwd.join("wide.png")).unwrap();
    let read = ReadTool::new(fs(), cwd, ReadOpts::default());
    let r = read
        .execute(
            cid(),
            serde_json::json!({ "path": "wide.png" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    let text = first_text(&r);
    assert!(
        text.contains("Image: original 2400x10, displayed at 2000x"),
        "got: {text}"
    );
    assert!(r.content.iter().any(|c| matches!(c, Content::Image { .. })));
}

// gap #5 — a non-zero exit with NO captured output labels the empty body `(no output)` and joins it
// via Pi's `appendStatus` (bash.ts:357,377,404-406): result is `"(no output)\n\nCommand exited with
// code 1"`, NOT a spurious leading `\n\n`.
#[tokio::test]
async fn bash_nonzero_exit_empty_output_labels_no_output() {
    let dir = tempfile::tempdir().unwrap();
    let bash = bash_tool(dir.path().to_path_buf(), BashOpts::default());
    let err = bash
        .execute(
            cid(),
            serde_json::json!({ "command": "exit 1" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert_eq!(
        msg, "(no output)\n\nCommand exited with code 1",
        "got: {msg}"
    );
}

// gap #5 — a timeout with NO captured output goes through the catch-path `formatOutput(snapshot, "")`
// (emptyText = ""), so `appendStatus` emits the bare status with NO leading `\n\n` (bash.ts:375,388).
#[tokio::test]
async fn bash_timeout_empty_output_has_no_leading_newline() {
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
    let msg = err.to_string();
    assert_eq!(msg, "Command timed out after 1 seconds", "got: {msg}");
}

// UM-5 (re-corrected) — Pi `getFileLines` folds lone `\r`→`\n` BEFORE splitting (grep.ts:206), so
// an interior CR is a LINE BREAK, not a character to strip. ripgrep numbers the whole
// `foo\rNEEDLE\rbar` as line 1 (it only breaks on `\n`), but Pi's folded src_lines are
// ["foo","NEEDLE","bar",""], so the rendered line-1 block is `foo` and its `context` neighbour is
// `NEEDLE`. `getFileLines` is reachable ONLY from `formatBlock`, i.e. only when `contextValue > 0`
// (grep.ts:328-330) — this test therefore passes `context`. The DEFAULT context=0 path takes
// `match.lineText` instead and is covered by `tests/grep_context_zero_line_text.rs`.
#[tokio::test]
async fn grep_folds_interior_carriage_returns_as_line_breaks() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    std::fs::write(cwd.join("cr.txt"), "foo\rNEEDLE\rbar\n").unwrap();
    let grep = GrepTool::new(fs(), cwd, GrepOpts::default());
    let r = grep
        .execute(
            cid(),
            serde_json::json!({ "pattern": "NEEDLE", "context": 1 }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    let text = first_text(&r);
    assert!(!text.contains('\r'), "no CR may survive folding: {text:?}");
    assert_eq!(
        text, "cr.txt:1: foo\ncr.txt-2- NEEDLE",
        "Pi renders the folded segments, not the joined line"
    );
}

// gap #9 — the edit access-error literal ends with a trailing period: Pi throws
// `Could not edit file: ${path}. ${errorMessage}.` (edit.ts:329).
#[tokio::test]
async fn edit_access_error_has_trailing_period() {
    let dir = tempfile::tempdir().unwrap();
    let edit = edit_tool(dir.path().to_path_buf());
    let err = edit
        .execute(
            cid(),
            serde_json::json!({ "path": "missing.txt", "edits": [{ "oldText": "a", "newText": "b" }] }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.starts_with("Could not edit file: missing.txt. "),
        "got: {msg}"
    );
    assert!(msg.ends_with('.'), "missing trailing period: {msg}");
}

// gap #7 — every non-bash tool's abort surfaces Pi's exact `"Operation aborted"` (capital O)
// literal (write.ts:209, edit.ts:319, ls.ts, read.ts:226, grep.ts, find.ts).
#[tokio::test]
async fn abort_message_is_capitalized() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    let cancel = CancelToken::new();
    cancel.cancel();
    let write = WriteTool::new(
        fs(),
        Arc::new(FileMutationLocks::new()),
        cwd,
        Default::default(),
    );
    let err = write
        .execute(
            cid(),
            serde_json::json!({ "path": "x.txt", "content": "hi" }),
            cancel,
            noop_sink(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "Operation aborted", "got: {err}");
}

// gap #2 — grep formats each match as an INDEPENDENT re-read block (grep.ts:250-268,316-331), so a
// context line shared by two matches within `2*context` lines is DUPLICATED (one copy per block),
// not merged. Two matches one line apart with context=1 share their middle line.
#[tokio::test]
async fn grep_context_blocks_duplicate_overlapping_lines() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    // Matches on lines 2 and 4; line 3 ("shared") is context for BOTH blocks.
    std::fs::write(
        cwd.join("f.txt"),
        "alpha\nNEEDLE one\nshared\nNEEDLE two\nomega\n",
    )
    .unwrap();
    let grep = GrepTool::new(fs(), cwd, GrepOpts::default());
    let r = grep
        .execute(
            cid(),
            serde_json::json!({ "pattern": "NEEDLE", "context": 1 }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    let text = first_text(&r);
    // Block for line 2: `f.txt-1- alpha`, `f.txt:2: NEEDLE one`, `f.txt-3- shared`.
    // Block for line 4: `f.txt-3- shared`, `f.txt:4: NEEDLE two`, `f.txt-5- omega`.
    let shared = text.matches("f.txt-3- shared").count();
    assert_eq!(
        shared, 2,
        "overlapping context line must appear once per block: {text:?}"
    );
    assert!(text.contains("f.txt:2: NEEDLE one"), "got: {text:?}");
    assert!(text.contains("f.txt:4: NEEDLE two"), "got: {text:?}");
}

// gap #3 — Pi clamps `effectiveLimit = Math.max(1, limit ?? 100)` (grep.ts:189); the JSON-schema
// `minimum:1` is advisory only. An explicit `limit: 0` must still return up to one match, never
// short-circuit to "No matches found".
#[tokio::test]
async fn grep_limit_zero_clamps_to_one_match() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    std::fs::write(cwd.join("f.txt"), "hit1\nhit2\nhit3\n").unwrap();
    let grep = GrepTool::new(fs(), cwd, GrepOpts::default());
    let r = grep
        .execute(
            cid(),
            serde_json::json!({ "pattern": "hit", "limit": 0 }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    let text = first_text(&r);
    assert_ne!(
        text, "No matches found",
        "limit:0 must clamp to 1, not short-circuit"
    );
    assert!(text.contains("f.txt:1: hit1"), "got: {text:?}");
    assert!(!text.contains("hit2"), "only one match expected: {text:?}");
    // count >= effectiveLimit(1) ⇒ the match-limit notice fires with `Use limit=2`.
    assert!(
        text.contains("1 matches limit reached. Use limit=2"),
        "got: {text:?}"
    );
}

// gap #4 — `read` prechecks EFFECTIVE `R_OK` (read.ts:54): an existing-but-unreadable candidate is
// skipped (not selected then failed at read time), yielding the synthesized "not found / unreadable"
// message. Skipped when the process bypasses `R_OK` (e.g. running as root), where the precheck is
// intentionally a no-op — matching Node's `fs.access` semantics.
#[cfg(unix)]
#[tokio::test]
async fn read_effective_access_skips_unreadable_candidate() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    let p = cwd.join("secret.txt");
    std::fs::write(&p, "top secret\n").unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o000)).unwrap();
    // If the test process can still read it, R_OK is bypassed (root); skip — same as Pi for root.
    if std::fs::read(&p).is_ok() {
        let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644));
        return;
    }
    let read = ReadTool::new(fs(), cwd, ReadOpts::default());
    let err = read
        .execute(
            cid(),
            serde_json::json!({ "path": "secret.txt" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap_err();
    // Pi re-rejects Node's raw `fs.access` error (read.ts:241 uncaught, :321-324 re-`reject`s), so
    // the model sees the errno class and the RESOLVED path — see `tests/read_access_errno.rs`.
    let msg = err.to_string();
    assert!(
        msg.contains("secret.txt"),
        "must name the resolved path: {msg}"
    );
    assert!(
        msg.contains("Permission denied"),
        "must carry the EACCES errno: {msg}"
    );
    let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644));
}

// gap #4 — `edit` prechecks EFFECTIVE `R_OK | W_OK` (edit.ts:86). A write-only file (mode 0o222) has
// its readonly BIT unset, so the old `permissions().readonly()` precheck would PASS and fail later
// at read time with a different message; the effective `R_OK` check now fails the precheck with Pi's
// `Could not edit file: {path}. {e}.`. Skipped when the process bypasses `R_OK` (root).
#[cfg(unix)]
#[tokio::test]
async fn edit_effective_access_precheck_rejects_write_only_file() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    let p = cwd.join("wo.txt");
    std::fs::write(&p, "hello\n").unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o222)).unwrap();
    if std::fs::read(&p).is_ok() {
        let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644));
        return;
    }
    let edit = edit_tool(cwd);
    let err = edit
        .execute(
            cid(),
            serde_json::json!({ "path": "wo.txt", "edits": [{ "oldText": "hello", "newText": "bye" }] }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap_err();
    assert!(
        err.to_string().starts_with("Could not edit file: wo.txt. "),
        "got: {err}"
    );
    let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644));
}

// ======================================================================================
// Round-13 UNTRACKED-miss byte-diff regressions (gap 04: UM-4..UM-7). Each asserts cyrup's
// model-facing output equals Pi's, with the exact Pi behavior reconstructed from source.
// ======================================================================================

// UM-5 (re-corrected) — grep `getFileLines` folds lone `\r`→`\n` BEFORE splitting (grep.ts:206).
// The matcher numbers lines on raw `\n`, so for a file using a LONE `\r` separator the CONTEXT
// BLOCK keys off the FOLDED segment. File bytes `x\rTARGET\n`: ripgrep (and grep_searcher) report
// the match on line 1 (`x\rTARGET`), but Pi's getFileLines splits the folded text into
// ["x","TARGET",""], so the block renders src_lines[0] = "x" as the match row and src_lines[1] =
// "TARGET" as its context row. `getFileLines` is only reachable via `formatBlock`, i.e. context>0
// (grep.ts:328-330) — the original version of this test omitted `context` and so asserted the
// folded rendering on the context=0 path, where Pi does not use it at all.
#[tokio::test]
async fn grep_folds_lone_cr_before_splitting_context() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    std::fs::write(cwd.join("cr.txt"), b"x\rTARGET\n").unwrap();
    let grep = GrepTool::new(fs(), cwd.clone(), GrepOpts::default());
    let r = grep
        .execute(
            cid(),
            serde_json::json!({ "pattern": "TARGET", "path": "cr.txt", "context": 1 }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    assert_eq!(
        first_text(&r),
        "cr.txt:1: x\ncr.txt-2- TARGET",
        "grep must fold lone CR like Pi getFileLines"
    );
}

// UM-4 — byte-limit NOTICE text hardcodes `formatSize(DEFAULT_MAX_BYTES)` (= 50.0KB) regardless of
// any configured limit (grep.ts:347). With a configured 10-byte limit the truncation fires, yet the
// notice must still read `50.0KB limit reached` (Pi's hardcoded constant), NOT `10B`.
#[tokio::test]
async fn grep_byte_limit_notice_uses_default_constant_not_configured() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    std::fs::write(
        cwd.join("f.txt"),
        "hello world this is a long matching line\n",
    )
    .unwrap();
    let grep = GrepTool::new(
        fs(),
        cwd.clone(),
        GrepOpts {
            limit: 100,
            max_bytes: 10,
            rg_config_path: None,
        },
    );
    let r = grep
        .execute(
            cid(),
            serde_json::json!({ "pattern": "hello", "path": "f.txt" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    let text = first_text(&r);
    assert!(
        text.contains("50.0KB limit reached"),
        "notice must use DEFAULT_MAX_BYTES: {text}"
    );
    assert!(
        !text.contains("10B limit reached"),
        "notice must NOT track configured limit: {text}"
    );
}

// UM-6 — read SELECTS the read-path variant by existence (F_OK) then checks readability (R_OK) on
// the CHOSEN path WITHOUT falling through to other variants (read.ts:238-241). A primary that
// EXISTS but is unreadable, with a readable curly-quote variant, must ERROR (Pi) — not silently
// succeed via the variant (old cyrup probed R_OK in the selection loop). `'` and U+2019 are
// distinct code points, so these are two real files even on case-insensitive APFS.
#[cfg(unix)]
#[tokio::test]
async fn read_variant_probe_uses_existence_not_readability() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    let primary = cwd.join("a'b.txt"); // straight apostrophe
    let variant = cwd.join("a\u{2019}b.txt"); // curly variant (resolve_read_path fallback)
    std::fs::write(&primary, "PRIMARY\n").unwrap();
    std::fs::write(&variant, "VARIANT\n").unwrap();
    std::fs::set_permissions(&primary, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read(&primary).is_ok() {
        // Running as root (R_OK bypassed): the precondition can't hold; skip.
        let _ = std::fs::set_permissions(&primary, std::fs::Permissions::from_mode(0o644));
        return;
    }
    let read = ReadTool::new(fs(), cwd.clone(), ReadOpts::default());
    let res = read
        .execute(
            cid(),
            serde_json::json!({ "path": "a'b.txt" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await;
    let _ = std::fs::set_permissions(&primary, std::fs::Permissions::from_mode(0o644));
    let err =
        res.expect_err("Pi errors: primary exists but is unreadable; no variant fall-through");
    // The failure must be EACCES on the PRIMARY — proving the selection loop chose it (F_OK) and
    // the readability check then failed on that same path rather than falling through to the
    // readable curly variant. Pi propagates Node's errno text (read.ts:241/321-324).
    let msg = err.to_string();
    assert!(msg.contains("Permission denied"), "got: {msg}");
    assert!(
        msg.contains("a'b.txt"),
        "must name the primary, not the variant: {msg}"
    );
    assert!(
        !msg.contains('\u{2019}'),
        "must not name the curly variant: {msg}"
    );
}

// UM-7 — a malformed `edits` (missing / non-array) yields Pi's exact `validateEditInput` literal
// (edit.ts:120-125), not a serde type error. Pi validates BEFORE deserialization.
#[tokio::test]
async fn edit_malformed_edits_yields_pi_literal() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    std::fs::write(cwd.join("f.txt"), "hello\n").unwrap();
    let edit = edit_tool(cwd);
    // edits is a number, not an array.
    let err = edit
        .execute(
            cid(),
            serde_json::json!({ "path": "f.txt", "edits": 42 }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Edit tool input is invalid. edits must contain at least one replacement."
    );
}

// ---------------------------------------------------------- numeric-parameter coercion (jsnum)
//
// Every numeric tool parameter is a TypeBox `Type.Number` upstream — no `integer`, no `minimum`
// (read.ts:22-23, grep.ts:31-34, ls.ts:16, find.ts:25) — and Pi never validates tool arguments at
// runtime: `wrapToolDefinition` (tool-definition-wrapper.ts:16-18) hands the model's parsed JSON
// straight to `execute`, which coerces at the point of use. So `10.0` and `-1` are inputs Pi
// accepts and answers. cyrup modeled them as `usize`, so `serde_json::from_value` rejected the
// whole call ("invalid type: floating point `10.0`" / "invalid value: integer `-1`") before the
// tool ran — a hard error where Pi returns a result. These tests pin the coerced behavior per
// tool, using Pi's own clamp expression as the oracle.

// read — `startLine = offset ? Math.max(0, offset - 1) : 0` (read.ts:271) and
// `endLine = Math.min(startLine + limit, allLines.length)` (read.ts:282).
#[tokio::test]
async fn read_accepts_float_and_negative_numeric_params() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    let ten = (1..=10)
        .map(|i| format!("line{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(cwd.join("f.txt"), &ten).unwrap();
    let read = ReadTool::new(fs(), cwd.clone(), ReadOpts::default());

    // Integral floats must behave exactly like the integer spelling.
    let float_args = read
        .execute(
            cid(),
            serde_json::json!({ "path": "f.txt", "offset": 2.0, "limit": 3.0 }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .expect("float offset/limit must not fail the call");
    let int_args = read
        .execute(
            cid(),
            serde_json::json!({ "path": "f.txt", "offset": 2, "limit": 3 }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    assert_eq!(first_text(&float_args), first_text(&int_args));
    assert!(first_text(&float_args).starts_with("line2\nline3\nline4"));

    // Negative offset: `Math.max(0, -5 - 1)` is 0, i.e. identical to reading from line 1.
    let neg = read
        .execute(
            cid(),
            serde_json::json!({ "path": "f.txt", "offset": -5 }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .expect("negative offset clamps to the start of the file, it does not fail the call");
    assert_eq!(first_text(&neg), ten);

    // Negative limit: Pi's unclamped `startLine + limit` selects nothing; cyrup clamps the window
    // end to `start` and still reports the remainder, so the model gets an actionable result.
    let neg_limit = read
        .execute(
            cid(),
            serde_json::json!({ "path": "f.txt", "limit": -1 }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .expect("negative limit must not fail the call");
    assert_eq!(
        first_text(&neg_limit),
        "\n\n[10 more lines in file. Use offset=1 to continue.]"
    );

    // The out-of-bounds message interpolates the RAW argument (read.ts:275); an integral float
    // renders without a fraction in both JS and Rust.
    let err = read
        .execute(
            cid(),
            serde_json::json!({ "path": "f.txt", "offset": 99.0 }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Offset 99 is beyond end of file (10 lines total)"
    );
}

// grep — `contextValue = context && context > 0 ? context : 0` and
// `effectiveLimit = Math.max(1, limit ?? DEFAULT_LIMIT)` (grep.ts:188-189).
#[tokio::test]
async fn grep_accepts_float_and_negative_numeric_params() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    std::fs::write(
        cwd.join("a.txt"),
        "one\nNEEDLE\nthree\nNEEDLE\nfive\nNEEDLE\n",
    )
    .unwrap();
    let grep = GrepTool::new(fs(), cwd.clone(), GrepOpts::default());

    // context: 1.0 must equal context: 1 — one line either side of the match.
    let ctx_float = grep
        .execute(
            cid(),
            serde_json::json!({ "pattern": "NEEDLE", "context": 1.0, "limit": 1 }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .expect("float context must not fail the call");
    assert!(
        first_text(&ctx_float).contains("a.txt-1- one"),
        "got: {}",
        first_text(&ctx_float)
    );

    // Negative context collapses to 0 — the match line alone, no surrounding rows.
    let ctx_neg = grep
        .execute(
            cid(),
            serde_json::json!({ "pattern": "NEEDLE", "context": -1, "limit": 1 }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .expect("negative context clamps to 0, it does not fail the call");
    assert_eq!(
        first_text(&ctx_neg).lines().next().unwrap(),
        "a.txt:2: NEEDLE"
    );
    assert!(
        !first_text(&ctx_neg).contains("a.txt-1-"),
        "got: {}",
        first_text(&ctx_neg)
    );

    // Negative limit is absorbed by `Math.max(1, …)`: exactly one match, not an error.
    let lim_neg = grep
        .execute(
            cid(),
            serde_json::json!({ "pattern": "NEEDLE", "limit": -1 }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .expect("negative limit clamps to 1, it does not fail the call");
    assert_eq!(
        first_text(&lim_neg)
            .lines()
            .filter(|l| l.contains("NEEDLE"))
            .count(),
        1
    );

    // limit: 2.0 must equal limit: 2.
    let lim_float = grep
        .execute(
            cid(),
            serde_json::json!({ "pattern": "NEEDLE", "limit": 2.0 }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .expect("float limit must not fail the call");
    assert_eq!(
        first_text(&lim_float)
            .lines()
            .filter(|l| l.contains("NEEDLE"))
            .count(),
        2
    );
}

// ls — `effectiveLimit = limit ?? DEFAULT_LIMIT` (ls.ts:125), unclamped: a non-positive limit
// satisfies `results.length >= effectiveLimit` immediately (ls.ts:156) so nothing is collected and
// Pi returns "(empty directory)".
#[tokio::test]
async fn ls_accepts_float_and_negative_limit() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    for n in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(cwd.join(n), "").unwrap();
    }
    let ls = LsTool::new(fs(), cwd.clone(), LsOpts::default());

    let float_limit = ls
        .execute(
            cid(),
            serde_json::json!({ "limit": 2.0 }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .expect("float limit must not fail the call");
    let text = first_text(&float_limit);
    assert!(
        text.starts_with("a.txt\nb.txt\n\n[2 entries limit reached."),
        "got: {text}"
    );

    let neg_limit = ls
        .execute(
            cid(),
            serde_json::json!({ "limit": -1 }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .expect("negative limit must not fail the call");
    assert_eq!(first_text(&neg_limit), "(empty directory)");
}

// find — `effectiveLimit = limit ?? DEFAULT_LIMIT` (find.ts:151) handed to `fd --max-results`
// (find.ts:241); a non-positive count produces no rows, which is Pi's "No files found" branch.
#[tokio::test]
async fn find_accepts_float_and_negative_limit() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    for n in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(cwd.join(n), "").unwrap();
    }
    let find = FindTool::new(fs(), cwd.clone(), FindOpts::default());

    let float_limit = find
        .execute(
            cid(),
            serde_json::json!({ "pattern": "*.txt", "limit": 2.0 }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .expect("float limit must not fail the call");
    let text = first_text(&float_limit);
    // Pi passes `--max-results N` to fd (find.ts:252) and NEVER sorts (`grep -n sort find.ts` is
    // empty at v0.84.1) — fd's traversal is parallel and unordered, so the result SET is "the
    // first N discovered", not "the alphabetically-first N". Asserting `a.txt\nb.txt` pinned
    // cyrup's own `results.sort()`, which was the defect (TOOL-023), so the property asserted here
    // is pi's: exactly `limit` rows, each a member of the candidate set, plus the notice.
    let (rows, notice) = text
        .split_once("\n\n[")
        .expect("notice bracket, got: {text}");
    let rows: Vec<&str> = rows.lines().collect();
    assert_eq!(
        rows.len(),
        2,
        "the cap must bound the row count, got: {text}"
    );
    for row in &rows {
        assert!(
            ["a.txt", "b.txt", "c.txt"].contains(row),
            "unexpected row {row}, got: {text}"
        );
    }
    assert!(
        notice.starts_with("2 results limit reached."),
        "got: {text}"
    );

    let neg_limit = find
        .execute(
            cid(),
            serde_json::json!({ "pattern": "*.txt", "limit": -1 }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .expect("negative limit must not fail the call");
    assert_eq!(first_text(&neg_limit), "No files found matching pattern");
}

// ---------------------------------------------------------------- powershell (Pi `powershell.ts`)

/// A `ProcOps` that records the [`ExecSpec`] it was handed and produces no output. The shell is
/// already resolved by the time it arrives, so this is the seam where "what the child would have
/// been given" is observable without a Windows host.
struct RecordingProc(std::sync::Mutex<Option<ExecSpec>>);

#[async_trait::async_trait]
impl ProcOps for RecordingProc {
    async fn exec(
        &self,
        spec: ExecSpec,
        _cancel: CancelToken,
        _timeout: Option<std::time::Duration>,
        _on_data: &mut (dyn for<'a> FnMut(&'a [u8]) + Send),
    ) -> Result<ExitStatus, ToolError> {
        *self.0.lock().unwrap() = Some(spec);
        Ok(ExitStatus::Exited(0))
    }
}

/// Pi's `UTF8_OUTPUT_PREFIX` (powershell.ts:16) is applied INSIDE `operations.exec`
/// (powershell.ts:35), i.e. AFTER `commandPrefix`, after `resolveSpawnContext` and after the
/// `spawnHook` (bash.ts:340-341,451). So the hook must observe the model's command WITHOUT the
/// preamble, and the preamble must appear exactly once, first, in what reaches the child.
///
/// Driven through [`ShellTool::new`] against a config that pairs PowerShell's preamble with a
/// resolvable shell, because `resolve_powershell` refuses off Windows before `exec` is reached.
#[tokio::test]
async fn the_utf8_preamble_goes_on_after_the_spawn_hook_and_only_once() {
    static PREAMBLE_CONFIG: crate::tools::ShellToolConfig = crate::tools::ShellToolConfig {
        name: "powershell",
        label: "powershell",
        shell_name: "PowerShell",
        prompt_snippet: "Execute PowerShell commands",
        prompt_guidelines: &[],
        temp_file_prefix: "cyrup-powershell",
        // Byte-identical to `powershell.rs`'s `UTF8_OUTPUT_PREFIX`, asserted below.
        command_preamble: Some(
            "try { [Console]::OutputEncoding=[System.Text.Encoding]::UTF8 } catch {}\n",
        ),
        resolve_shell: crate::ops::ShellConfig::resolve,
    };

    let seen_by_hook: Arc<std::sync::Mutex<Option<String>>> = Arc::new(std::sync::Mutex::new(None));
    let recorder = Arc::new(RecordingProc(std::sync::Mutex::new(None)));
    let dir = tempfile::tempdir().unwrap();
    let opts = BashOpts {
        spawn_hook: {
            let seen = seen_by_hook.clone();
            Some(Arc::new(move |ctx: crate::config::BashSpawnContext| {
                *seen.lock().unwrap() = Some(ctx.command.clone());
                ctx
            }))
        },
        ..BashOpts::default()
    };
    let tool = ShellTool::new(
        &PREAMBLE_CONFIG,
        recorder.clone(),
        dir.path().to_path_buf(),
        opts,
    );
    tool.execute(
        cid(),
        serde_json::json!({ "command": "Get-ChildItem" }),
        CancelToken::new(),
        noop_sink(),
    )
    .await
    .unwrap();

    assert_eq!(
        seen_by_hook.lock().unwrap().as_deref(),
        Some("Get-ChildItem"),
        "the spawn hook sees the model's command WITHOUT the preamble (powershell.ts:35 runs later)"
    );
    let command = recorder.0.lock().unwrap().as_ref().unwrap().command.clone();
    assert_eq!(
        command,
        "try { [Console]::OutputEncoding=[System.Text.Encoding]::UTF8 } catch {}\nGet-ChildItem",
        "the preamble is prepended exactly once, immediately before the child is built"
    );
    // The literal above is the one `tools/powershell.rs` ships, reached through the real config.
    assert_eq!(
        crate::tools::POWERSHELL_CONFIG.command_preamble,
        PREAMBLE_CONFIG.command_preamble
    );
}

/// A `bash` command must be byte-identical to what it was before `powershell` existed: no preamble
/// (`bashToolConfig` sets none, bash.ts:519-527).
#[tokio::test]
async fn bash_commands_get_no_preamble() {
    let recorder = Arc::new(RecordingProc(std::sync::Mutex::new(None)));
    let dir = tempfile::tempdir().unwrap();
    let bash = ShellTool::bash(
        recorder.clone(),
        dir.path().to_path_buf(),
        BashOpts::default(),
    );
    bash.execute(
        cid(),
        serde_json::json!({ "command": "echo hi" }),
        CancelToken::new(),
        noop_sink(),
    )
    .await
    .unwrap();
    assert_eq!(
        recorder.0.lock().unwrap().as_ref().unwrap().command,
        "echo hi"
    );
    assert!(crate::tools::BASH_CONFIG.command_preamble.is_none());
}

/// Off Windows, every `powershell` call is Pi's `win32` throw (shell.ts:127) surfaced as the tool
/// result — bare: no prefix, no `Command exited with code …` / `Command aborted` suffix, no output
/// body. The refusal comes from the shell thunk, so it happens without touching the process seam.
#[cfg(not(windows))]
#[tokio::test]
async fn powershell_off_windows_returns_pis_bare_refusal() {
    let recorder = Arc::new(RecordingProc(std::sync::Mutex::new(None)));
    let dir = tempfile::tempdir().unwrap();
    let ps = ShellTool::powershell(
        recorder.clone(),
        dir.path().to_path_buf(),
        PowerShellOpts::default(),
    );
    let err = ps
        .execute(
            cid(),
            serde_json::json!({ "command": "Get-ChildItem" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .expect_err("off Windows the shell thunk throws");
    assert_eq!(
        err.to_string(),
        "The powershell tool is only available on Windows."
    );
    assert!(
        recorder.0.lock().unwrap().is_none(),
        "the refusal precedes the process seam entirely"
    );
}

/// The `shellPath` setting names a BASH. `PowerShellOpts` cannot carry it and cannot carry a
/// `shellCommandPrefix` either, and the widening to [`BashOpts`] pins both to `None`
/// (powershell.ts:29-30,53-56) — so a `shellPath` pointing anywhere can only ever steer `bash`.
#[tokio::test]
async fn powershell_options_can_never_carry_shell_path_or_command_prefix() {
    let widened = BashOpts::from(PowerShellOpts {
        bin_dir: Some(PathBuf::from("/opt/cyrup/bin")),
        ..PowerShellOpts::default()
    });
    assert!(widened.shell_path.is_none());
    assert!(widened.command_prefix.is_none());
    assert_eq!(widened.bin_dir, Some(PathBuf::from("/opt/cyrup/bin")));
    assert!(widened.expose_session_environment);
}

/// Pi's missing-cwd message names the SHELL (`Cannot execute ${shellName} commands.`, bash.ts:95).
/// The name rides on the resolved [`crate::ops::ShellConfig`], so the process backend can say
/// `PowerShell` for one tool and `bash` for the other from a single code path.
#[tokio::test]
async fn the_missing_cwd_error_names_the_shell() {
    let missing = std::env::temp_dir().join("cyrup-no-such-cwd-for-the-shell-name-test");
    let _ = std::fs::remove_dir_all(&missing);
    let mut sink = |_: &[u8]| {};

    for (shell_name, expected) in [("bash", "bash"), ("PowerShell", "PowerShell")] {
        let spec = ExecSpec {
            command: "noop".to_string(),
            cwd: missing.clone(),
            env: Vec::new(),
            env_remove: Vec::new(),
            shell: crate::ops::ShellConfig {
                shell_name,
                program: PathBuf::from("/bin/sh"),
                args: vec!["-c".to_string()],
                transport: crate::ops::Transport::Argv,
            },
        };
        let err = proc()
            .exec(spec, CancelToken::new(), None, &mut sink)
            .await
            .expect_err("a missing cwd is Pi's actionable error, not a raw spawn failure");
        assert_eq!(
            err.to_string(),
            format!(
                "Working directory does not exist: {}\nCannot execute {expected} commands.",
                missing.display()
            )
        );
    }
}

/// fd sorts its results when the cap fires while it is still buffering, and cyrup did not.
///
/// `ReceiverBuffer::stop` (fd 10.5.0 `walk.rs:281-285`) calls `self.buffer.sort()` before emitting
/// whenever the mode is still `Buffering`, which is the case under pi's own default —
/// `--max-results 1000` (find.ts:44, :165, :252) against `MAX_BUFFER_LENGTH` of 1000
/// (`walk.rs:125`). So for any tree that answers inside fd's 100 ms deadline, pi's caller sees
/// sorted paths. cyrup returned `ignore::Walk` readdir order.
///
/// The fixture pins BOTH halves of the fix at once. fd orders by
/// `self.path().cmp(other.path())` (`dir_entry.rs:132-136`) — a COMPONENT comparison — so:
///
///   `Path::cmp`  ->  ["a", "b.txt"] < ["a.txt"]   because "a" < "a.txt"     =>  a/b.txt first
///   `str::cmp`   ->  "a.txt" < "a/b.txt"          because '.' (0x2E) < '/'  =>  a.txt first
///
/// The two disagree, so this test fails if the sort is missing AND fails if it is done byte-wise
/// on the joined strings. Any name holding a character below `/` — `.`, `-`, space — flips it,
/// which covers dotfiles and hyphenated names.
#[tokio::test]
async fn find_returns_fd_sorted_order_not_readdir_order() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    std::fs::create_dir(cwd.join("a")).unwrap();
    // Created in an order that is neither the sorted nor the reverse-sorted one, so a pass cannot
    // come from the filesystem happening to hand them back already ordered.
    for rel in ["z.txt", "a/b.txt", "a.txt"] {
        std::fs::write(cwd.join(rel), "x").unwrap();
    }
    let find = FindTool::new(fs(), cwd, FindOpts { limit: 1000, max_bytes: 50 * 1024 });
    let r = find
        .execute(
            cid(),
            // `*.txt` has no `/`, so it is basename-matched and the `a` directory is not a result.
            serde_json::json!({ "pattern": "*.txt" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();

    assert_eq!(
        first_text(&r),
        "a/b.txt\na.txt\nz.txt",
        "fd sorts by Path components when the cap fires while buffering"
    );
}
