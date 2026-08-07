//! `compute_edits_diff` — the port of Pi's `computeEditsDiff` (edit-diff.ts:514-547), the
//! pre-execution preview `edit`'s `renderCall` fires once the streamed arguments are complete
//! (edit.ts:377-386).
//!
//! The one invariant that matters beyond "it produces a diff": the preview must be **byte-identical**
//! to the `details.diff` the tool itself later reports, and it must reach that answer **without
//! writing anything**. Those two together are what let the renderer draw the diff once, before the
//! write, and then keep it (`formatEditResult` returns nothing when the two agree, edit.ts:220-226).
//! If they ever diverged, the transcript would flicker from one diff to a different one the instant
//! the tool settled.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use cyrup_core::{CancelToken, Tool, ToolCallId, ToolUpdate, ToolUpdateSink};
use cyrup_tools::ops::local::LocalFs;
use cyrup_tools::ops::FsOps;
use cyrup_tools::tools::edit_diff::compute_edits_diff;
use cyrup_tools::tools::EditTool;
use cyrup_tools::FileMutationLocks;
use std::sync::Arc;

fn edit_tool(cwd: std::path::PathBuf) -> EditTool {
    let fs: Arc<dyn FsOps> = Arc::new(LocalFs);
    EditTool::new(fs, Arc::new(FileMutationLocks::new()), cwd, Default::default())
}

fn noop_sink() -> ToolUpdateSink {
    Box::new(|_u: ToolUpdate| {})
}

/// Run the real tool and return its `details.diff`.
async fn tool_diff(cwd: &std::path::Path, args: serde_json::Value) -> String {
    let r = edit_tool(cwd.to_path_buf())
        .execute(ToolCallId::from("tc"), args, CancelToken::new(), noop_sink())
        .await
        .unwrap();
    r.details
        .as_ref()
        .and_then(|d| d.get("diff"))
        .and_then(serde_json::Value::as_str)
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn preview_matches_the_tools_own_diff_and_writes_nothing() {
    // CRLF + a BOM, because that is where a "just diff the file" shortcut would drift from the
    // tool: both sides normalize to LF before diffing and the preview must make the same choice.
    let before = "\u{feff}one\r\ntwo\r\nthree\r\nfour\r\n";
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("f.txt");
    std::fs::write(&file, before).unwrap();

    let edits = [("two".to_string(), "TWO".to_string()), ("four".to_string(), "FOUR".to_string())];
    let preview = compute_edits_diff("f.txt", &edits, dir.path()).unwrap();

    assert!(!preview.diff.is_empty(), "no diff produced");
    assert_eq!(preview.first_changed_line, Some(2));
    assert_eq!(
        std::fs::read(&file).unwrap(),
        before.as_bytes(),
        "the preview wrote to the file it was only supposed to read"
    );

    let args = serde_json::json!({
        "path": "f.txt",
        "edits": [
            { "oldText": "two", "newText": "TWO" },
            { "oldText": "four", "newText": "FOUR" },
        ],
    });
    assert_eq!(
        preview.diff,
        tool_diff(dir.path(), args).await,
        "preview and settled diff must agree byte-for-byte, or the transcript flickers"
    );
    // And the tool DID write — proof the comparison above was against a real execution.
    assert_ne!(std::fs::read(&file).unwrap(), before.as_bytes(), "the tool did not write");
}

/// Failures come back with the message the tool itself would have produced, so the preview and the
/// eventual error read identically (which is what lets the renderer suppress the duplicate).
#[tokio::test]
async fn failures_carry_the_tools_own_wording() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.txt"), "one\ntwo\n").unwrap();

    let missing = compute_edits_diff(
        "f.txt",
        &[("nowhere".to_string(), "x".to_string())],
        dir.path(),
    )
    .unwrap_err();
    assert!(
        missing.starts_with("Could not find the exact text in f.txt."),
        "unexpected not-found wording: {missing}"
    );

    // Pi's `Could not edit file: {path}. {…}.` for an unreadable target (edit-diff.ts:527-531).
    let absent =
        compute_edits_diff("nope.txt", &[("a".to_string(), "b".to_string())], dir.path())
            .unwrap_err();
    assert!(
        absent.starts_with("Could not edit file: nope.txt."),
        "unexpected unreadable wording: {absent}"
    );
    assert!(absent.ends_with('.'), "Pi's message ends in a period: {absent}");

    // A replacement that changes nothing is an error upstream too, not an empty diff.
    let noop =
        compute_edits_diff("f.txt", &[("two".to_string(), "two".to_string())], dir.path())
            .unwrap_err();
    assert!(noop.starts_with("No changes made to f.txt."), "unexpected no-op wording: {noop}");
}
