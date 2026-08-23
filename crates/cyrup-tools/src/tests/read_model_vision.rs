//! `read`'s non-vision note must track the model active AT CALL TIME (A-03-2, R-03-012).
//!
//! Pi computes it per execution off the per-call `ExtensionContext`:
//! `const nonVisionImageNote = getNonVisionImageNote(ctx?.model);` (pi v0.83.0
//! `packages/coding-agent/src/core/tools/read.ts:246`, over `model.input.includes("image")` at
//! read.ts:87-92), appended to the tool text at read.ts:253 and read.ts:258. A mid-session `/model`
//! switch therefore changes the very next `read`.
//!
//! cyrup's `Tool::execute` takes no context, so the capability arrives as a shared
//! [`ModelVisionHandle`] the session layer mutates on `set_model` — the same shape `bash` already
//! uses for its session metadata (`tests/bash_session_env.rs`). These tests pin the handle down as
//! the source of truth, because the construction-time `ReadOpts::supports_images` field it replaces
//! was never derived from any model and made the note dead code in every real session.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::config::{ModelVisionHandle, ReadOpts};
use crate::ops::FsOps;
use crate::ops::local::LocalFs;
use crate::tools::ReadTool;
use cyrup_core::{CancelToken, Content, Tool, ToolCallId, ToolResult, ToolUpdate, ToolUpdateSink};
use std::sync::Arc;

const NON_VISION_NOTE: &str =
    "[Current model does not support images. The image will be omitted from this request.]";

fn fs() -> Arc<dyn FsOps> {
    Arc::new(LocalFs)
}

fn cid() -> ToolCallId {
    ToolCallId::from("tc-vision")
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

/// Flipping the live handle after the tool exists must flip the note — this is the whole point of
/// the handle, and the behaviour the frozen `ReadOpts::supports_images` field could not provide.
///
/// Feature-agnostic on purpose: the `inline-images` build appends the note to the processed-image
/// text (read.rs, Pi read.ts:258) and the `--no-default-features` build appends it to the
/// build-note text, so both carry it.
#[tokio::test]
async fn read_non_vision_note_follows_live_model_switch() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    // Detection is by MAGIC BYTES (mime.ts), so this has to be a real PNG.
    let img = image::RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 255]));
    img.save(cwd.join("pic.png")).unwrap();

    // Session starts on a vision model. `supports_images: false` is the static fallback and is set
    // to the OPPOSITE of the handle throughout, so any assertion that passes can only be reading
    // the handle.
    let vision = ModelVisionHandle::new(true);
    let read = ReadTool::new(
        fs(),
        cwd,
        ReadOpts {
            supports_images: false,
            model_vision: Some(vision.clone()),
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
        !text.contains(NON_VISION_NOTE),
        "vision model must not get the non-vision note; got: {text}"
    );

    // `/model` switch to a text-only model, mid-session, with the tool already constructed.
    vision.set(false);

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
        text.contains(NON_VISION_NOTE),
        "note must appear on the very next read after the switch; got: {text}"
    );

    // ...and back again, proving the read is live in both directions rather than latched.
    vision.set(true);

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
        !text.contains(NON_VISION_NOTE),
        "switching back must drop the note; got: {text}"
    );
}

/// With no session layer wired (`model_vision: None`) the static field still decides, matching how
/// `BashOpts::session_env == None` degrades to Pi's `ctx === undefined`.
#[tokio::test]
async fn read_falls_back_to_static_flag_without_a_handle() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    let img = image::RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 255]));
    img.save(cwd.join("pic.png")).unwrap();

    let read = ReadTool::new(
        fs(),
        cwd,
        ReadOpts {
            supports_images: false,
            model_vision: None,
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
    assert!(first_text(&r).contains(NON_VISION_NOTE));

    assert!(
        ReadOpts::default().supports_images_now(),
        "default stays vision-capable"
    );
}
