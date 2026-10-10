//! The structured half of `read` and `bash` results, which a programmatic caller such as a
//! codemode script receives instead of the model-facing text (pi `021eae60a` @v1.0.4, TOOL-054,
//! TOOL-058).
//!
//! Upstream declares an `outputSchema` for exactly these two tools (`core/tools/read.ts:33-38`,
//! `core/tools/bash.ts:55-62`; `git grep outputSchema v1.0.4 -- core/tools` finds no other built-in)
//! and sets `structuredContent` on every successful result, with a non-zero `bash` exit returned as
//! an `isError` result so the structured value survives it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::config::{BashOpts, ReadOpts};
use crate::ops::local::LocalFs;
use crate::ops::{Backend, FsOps};
use crate::tools::{ReadTool, ShellTool};
use cyrup_core::{CancelToken, Content, Tool, ToolCallId, ToolResult, ToolUpdate, ToolUpdateSink};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

fn cid() -> ToolCallId {
    ToolCallId::from("tc-structured")
}

fn noop_sink() -> ToolUpdateSink {
    Box::new(|_u: ToolUpdate| {})
}

fn first_text(r: &ToolResult) -> String {
    r.content
        .iter()
        .find_map(|block| match block {
            Content::Text { text, .. } => Some(text.to_string()),
            _ => None,
        })
        .unwrap_or_default()
}

fn bash(cwd: PathBuf) -> ShellTool {
    ShellTool::bash(Backend::default().proc, cwd, BashOpts::default())
}

fn read(cwd: PathBuf) -> ReadTool {
    let fs: Arc<dyn FsOps> = Arc::new(LocalFs);
    ReadTool::new(fs, cwd, ReadOpts::default())
}

async fn run_bash(command: &str) -> ToolResult {
    let dir = tempfile::tempdir().unwrap();
    bash(dir.path().to_path_buf())
        .execute(
            cid(),
            json!({ "command": command }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .expect("a command that ran is a result, whatever its exit code")
}

fn structured(result: &ToolResult) -> &Value {
    result
        .structured_content
        .as_ref()
        .expect("the result carries structured content")
}

#[test]
fn bash_declares_its_output_schema() {
    let tool = bash(PathBuf::from("."));
    let schema = tool
        .output_schema()
        .expect("bash declares an output schema");
    let properties = schema["properties"].as_object().unwrap();
    assert_eq!(
        properties.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "output",
            "truncated",
            "full_output_path",
            "exit_code",
            "wall_time_seconds"
        ]
    );
    assert_eq!(
        schema["required"],
        json!(["output", "truncated", "exit_code", "wall_time_seconds"])
    );
}

#[tokio::test]
async fn bash_success_carries_the_structured_result() {
    let result = run_bash("echo out").await;
    assert!(!result.is_error);
    assert_eq!(first_text(&result), "out\n");
    let value = structured(&result);
    assert_eq!(value["output"], "out\n");
    assert_eq!(value["truncated"], false);
    assert_eq!(value["exit_code"], 0);
    assert!(value["wall_time_seconds"].is_number(), "{value}");
    assert!(value.get("full_output_path").is_none(), "{value}");
}

/// The upstream spec `resolves bash calls to structured results, also for non-zero exit codes`
/// (`agent-session-codemode.test.ts`): `echo out; exit 3` resolves to `["out\n", 3, "number"]`.
/// The model reads the text a throw produced, as an error result.
#[tokio::test]
async fn bash_nonzero_exit_is_an_error_result_with_the_structured_result() {
    let result = run_bash("echo out; exit 3").await;
    assert!(result.is_error);
    assert_eq!(first_text(&result), "out\n\n\nCommand exited with code 3");
    let value = structured(&result);
    assert_eq!(value["output"], "out\n");
    assert_eq!(value["exit_code"], 3);
    assert!(value["wall_time_seconds"].is_number(), "{value}");
    assert_eq!(result.details.as_ref().unwrap()["exitCode"], 3);
}

/// `output` is the whole output where the model-facing text is the last 2000 lines
/// (`bash.ts:381-400`: `readFullOutput` over the spill file).
#[tokio::test]
async fn bash_structured_output_is_not_limited_to_the_preview() {
    let result = run_bash("seq 1 3000").await;
    let preview = first_text(&result);
    assert!(preview.contains("[Showing lines"), "{preview}");
    assert!(!preview.starts_with("1\n"), "the preview is the tail");
    let value = structured(&result);
    let output = value["output"].as_str().unwrap();
    assert!(output.starts_with("1\n2\n"), "the whole output starts at 1");
    assert!(output.ends_with("2999\n3000\n"));
    assert_eq!(output.lines().count(), 3000);
    assert_eq!(value["truncated"], false);
    assert!(value.get("full_output_path").is_none(), "{value}");
}

/// Past 1 MiB the structured output keeps its first and last 512 KiB around an omission marker,
/// says so, and names the file holding all of it (`output-accumulator.ts:125-150`).
#[tokio::test]
async fn bash_structured_output_over_one_mebibyte_keeps_head_and_tail() {
    let result = run_bash("yes 0123456789abcdef | head -c 3000000; echo END").await;
    let value = structured(&result);
    assert_eq!(value["truncated"], true);
    let output = value["output"].as_str().unwrap();
    assert!(output.starts_with("0123456789abcdef\n"));
    assert!(output.ends_with("END\n"));
    assert!(output.contains("bytes omitted ...]"), "marker missing");
    assert!(output.len() < 1024 * 1024 + 100, "got {}", output.len());
    let path = value["full_output_path"].as_str().unwrap();
    assert!(
        first_text(&result).contains(path),
        "the preview names the same file"
    );
    let on_disk = std::fs::metadata(path).unwrap().len();
    let _ = std::fs::remove_file(path);
    assert_eq!(on_disk, 3_000_004);
}

#[test]
fn read_declares_text_or_an_image_block() {
    let tool = read(PathBuf::from("."));
    let schema = tool
        .output_schema()
        .expect("read declares an output schema");
    let variants = schema["anyOf"].as_array().unwrap();
    assert_eq!(variants[0], json!({ "type": "string" }));
    assert_eq!(
        variants[1]["required"],
        json!(["type", "data", "mimeType", "note"])
    );
    assert_eq!(variants[1]["properties"]["type"]["const"], "image");
}

#[tokio::test]
async fn read_text_resolves_to_the_text() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("notes.txt"), "hello").unwrap();
    let result = read(dir.path().to_path_buf())
        .execute(
            cid(),
            json!({ "path": "notes.txt" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    assert_eq!(first_text(&result), "hello");
    assert_eq!(structured(&result), &json!("hello"));
}

/// The continuation note of a windowed read is part of the text, so it is part of the structured
/// value too: `toReadOutput` takes the first text block as it is.
#[tokio::test]
async fn read_window_notes_are_part_of_the_structured_text() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lines.txt"), "a\nb\nc\nd").unwrap();
    let result = read(dir.path().to_path_buf())
        .execute(
            cid(),
            json!({ "path": "lines.txt", "limit": 2 }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    let text = first_text(&result);
    assert!(text.contains("more lines in file"), "{text}");
    assert_eq!(structured(&result), &Value::String(text));
}

#[tokio::test]
async fn read_image_resolves_to_the_image_block_with_its_note() {
    let dir = tempfile::tempdir().unwrap();
    let pixel = image::RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 255]));
    pixel.save(dir.path().join("pixel.png")).unwrap();
    let result = read(dir.path().to_path_buf())
        .execute(
            cid(),
            json!({ "path": "pixel.png" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    let (data, mime_type) = result
        .content
        .iter()
        .find_map(|block| match block {
            Content::Image { data, mime_type } => Some((data.clone(), mime_type.clone())),
            _ => None,
        })
        .expect("the model gets the image block");
    assert_eq!(
        structured(&result),
        &json!({
            "type": "image",
            "data": data,
            "mimeType": mime_type,
            "note": first_text(&result),
        })
    );
    assert!(
        first_text(&result).starts_with("Read image file [image/png]"),
        "{}",
        first_text(&result)
    );
}
