//! `grep` at the DEFAULT `context` (0) must render the matched line from ripgrep's own captured
//! line text, not from a folded re-read of the file.
//!
//! Pi (`coding-agent/src/core/tools/grep.ts:317-327`):
//!
//! ```js
//! if (contextValue === 0 && match.lineText !== undefined) {
//!     const sanitized = match.lineText
//!         .replace(/\r\n/g, "\n")
//!         .replace(/\r/g, "")
//!         .replace(/\n$/, "");
//!     ...
//!     outputLines.push(`${relativePath}:${match.lineNumber}: ${truncatedText}`);
//! } else {
//!     const block = await formatBlock(match.filePath, match.lineNumber);
//! }
//! ```
//!
//! `contextValue` defaults to 0 (grep.ts:158), so this is the ordinary `grep` call. The lone-`\r`→
//! `\n` fold at grep.ts:206 lives in `getFileLines`, reachable ONLY from `formatBlock` — the
//! `else` branch. cyrup used to build the folded split unconditionally and index it for every
//! match, so on a file with lone-`\r` separators it printed text from a DIFFERENT position than the
//! line number it reported (`x\rTARGET` → `cr.txt:1: x`, which does not even contain the pattern).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::config::GrepOpts;
use crate::ops::local::LocalFs;
use crate::tools::GrepTool;
use cyrup_core::{CancelToken, Content, Tool, ToolCallId, ToolResult, ToolUpdate};
use std::sync::Arc;

fn first_text(r: &ToolResult) -> String {
    match r.content.first() {
        Some(Content::Text { text, .. }) => text.to_string(),
        _ => String::new(),
    }
}

/// Run `grep` over a one-file tempdir. Returns the model-facing text.
async fn grep_bytes(name: &str, bytes: &[u8], args: serde_json::Value) -> String {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    std::fs::write(cwd.join(name), bytes).unwrap();
    let grep = GrepTool::new(Arc::new(LocalFs), cwd, GrepOpts::default());
    let r = grep
        .execute(
            ToolCallId::from("tc-test"),
            args,
            CancelToken::new(),
            Box::new(|_u: ToolUpdate| {}),
        )
        .await
        .unwrap();
    // Hold the tempdir until after the call so the walk cannot race its own cleanup.
    drop(dir);
    first_text(&r)
}

/// The headline divergence. Bytes `x\rTARGET\n` are ONE line to ripgrep (it breaks on `\n` only),
/// so `lineText` is `"x\rTARGET\n"` and Pi's sanitiser DELETES the `\r` (it does not fold it to a
/// newline) and strips the single trailing `\n`, yielding `cr.txt:1: xTARGET`. The old cyrup folded
/// `\r`→`\n`, split, and rendered `src_lines[0]` = `x` — content from a different position than the
/// reported line number, and not even containing the searched term.
#[tokio::test]
async fn lone_cr_line_renders_ripgrep_line_text_not_a_folded_segment() {
    let text = grep_bytes(
        "cr.txt",
        b"x\rTARGET\n",
        serde_json::json!({ "pattern": "TARGET" }),
    )
    .await;
    assert_eq!(text, "cr.txt:1: xTARGET");
    assert!(
        !text.contains('\r'),
        "Pi deletes every surviving CR: {text:?}"
    );
}

/// Several interior lone `\r`s, all deleted rather than folded.
#[tokio::test]
async fn interior_lone_crs_are_deleted_not_folded_at_default_context() {
    let text = grep_bytes(
        "cr.txt",
        b"foo\rNEEDLE\rbar\n",
        serde_json::json!({ "pattern": "NEEDLE" }),
    )
    .await;
    assert_eq!(text, "cr.txt:1: fooNEEDLEbar");
}

/// An EXPLICIT `context: 0` is the same branch as the default (`contextValue === 0`).
#[tokio::test]
async fn explicit_zero_context_takes_the_same_branch_as_the_default() {
    let text = grep_bytes(
        "cr.txt",
        b"x\rTARGET\n",
        serde_json::json!({ "pattern": "TARGET", "context": 0 }),
    )
    .await;
    assert_eq!(text, "cr.txt:1: xTARGET");
}

/// `context: 1` must still go through `formatBlock`/`getFileLines`, where the lone-`\r` DOES fold
/// into a line break (grep.ts:206). Same bytes, deliberately different rendering — this is the pair
/// that pins which branch owns which sanitisation.
#[tokio::test]
async fn nonzero_context_still_folds_lone_cr_through_get_file_lines() {
    let text = grep_bytes(
        "cr.txt",
        b"x\rTARGET\n",
        serde_json::json!({ "pattern": "TARGET", "context": 1 }),
    )
    .await;
    assert_eq!(text, "cr.txt:1: x\ncr.txt-2- TARGET");
}

/// CRLF files must be unaffected: `\r\n`→`\n` then the trailing-`\n` strip leaves a bare line, the
/// same answer the folded re-read gave. Guards against "fixing" the CR handling by deleting the
/// `\r\n` normalisation.
#[tokio::test]
async fn crlf_line_endings_are_unchanged() {
    let text = grep_bytes(
        "crlf.txt",
        b"hello\r\nworld\r\n",
        serde_json::json!({ "pattern": "hello" }),
    )
    .await;
    assert_eq!(text, "crlf.txt:1: hello");
}

/// Only ONE trailing newline is stripped (`replace(/\n$/,"")`), and a final line with no terminator
/// at all renders whole.
#[tokio::test]
async fn last_line_without_a_terminator_renders_whole() {
    let text = grep_bytes(
        "t.txt",
        b"a\nNEEDLE",
        serde_json::json!({ "pattern": "NEEDLE" }),
    )
    .await;
    assert_eq!(text, "t.txt:2: NEEDLE");
}

/// ripgrep emits `lines.bytes` (base64) instead of `lines.text` when the matched line is not valid
/// UTF-8, leaving `match.lineText` undefined — so Pi's condition fails and even at context 0 the
/// match falls through to `formatBlock` (grep.ts:318,328). That path decodes lossily, so the
/// replacement character appears exactly as Pi's `readFile(path,"utf8")` produces it.
#[tokio::test]
async fn non_utf8_matched_line_falls_back_to_the_format_block_path() {
    let text = grep_bytes(
        "l1.txt",
        b"caf\xe9 NEEDLE\n",
        serde_json::json!({ "pattern": "NEEDLE" }),
    )
    .await;
    assert_eq!(text, "l1.txt:1: caf\u{fffd} NEEDLE");
}
