//! `read` resizes to the MODEL's profile, not to a hardcoded 2000px (PROV-134's read-tool clause,
//! and the literal text of CFG-085's **Verify**).
//!
//! Pi reads the profile per call off the live context model:
//! `resizeOptions: ctx?.model?.inputLimits?.images?.resize ?? fallbackResizeOptions`
//! (`packages/coding-agent/src/core/tools/read.ts:138` @v1.0.4, re-read via `git -C tmp/pi show`).
//! cyrup's `ReadOpts` previously carried `max_image_dim: u32` — ONE number, with no way to express
//! a non-square profile, no `maxBytes` and no `jpegQuality` at all — so a model row declaring
//! `{"inputLimits":{"images":{"resize":{"maxWidth":512}}}}` was unreachable from `read` by
//! construction. These tests pin the field that replaces it as the thing the resizer actually
//! reads, and pin `None` to the 2000px default so the replacement changed no behaviour.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::config::ReadOpts;
use crate::ops::FsOps;
use crate::ops::local::LocalFs;
use crate::tools::ReadTool;
use cyrup_core::{CancelToken, Content, ModelImageResizeOptions, Tool, ToolCallId, ToolUpdate};
use std::sync::Arc;

/// A 2600x800 PNG of fixed-seed noise — wide enough that both a 2000px and a 512px clamp bind, and
/// incompressible enough that the result is attributable to the clamp rather than to the byte cap.
fn wide_noise_png(path: &std::path::Path) {
    let mut state: u32 = 0x1234_5678;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    let img: image::RgbImage = image::ImageBuffer::from_fn(2600, 800, |_, _| {
        let n = next();
        image::Rgb([n as u8, (n >> 8) as u8, (n >> 16) as u8])
    });
    img.save(path).unwrap();
}

async fn read_png_width(profile: Option<ModelImageResizeOptions>) -> u32 {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    wide_noise_png(&cwd.join("shot.png"));

    let read = ReadTool::new(
        Arc::new(LocalFs) as Arc<dyn FsOps>,
        cwd,
        ReadOpts {
            image_resize: profile,
            ..ReadOpts::default()
        },
    );
    let r = read
        .execute(
            ToolCallId::from("tc-resize"),
            serde_json::json!({ "path": "shot.png" }),
            CancelToken::new(),
            Box::new(|_u: ToolUpdate| {}),
        )
        .await
        .unwrap();

    let Some(Content::Image { data, .. }) = r
        .content
        .iter()
        .find(|c| matches!(c, Content::Image { .. }))
        .cloned()
    else {
        panic!("read returned no image block: {:?}", r.content);
    };
    let bytes = crate::image_proc::base64_decode_for_tests(&data);
    image::load_from_memory(&bytes).expect("decodable").width()
}

/// The profile on `ReadOpts` reaches the resizer: a 512px `maxWidth` really clamps `read`'s output
/// to 512px, which `max_image_dim` could only have done by being set to 512 globally.
#[tokio::test]
async fn read_honours_the_models_resize_profile() {
    let w = read_png_width(Some(ModelImageResizeOptions {
        max_width: Some(512),
        ..ModelImageResizeOptions::default()
    }))
    .await;
    assert_eq!(w, 512, "read must clamp to the model's maxWidth");
}

/// And no profile keeps the old number exactly, so replacing `max_image_dim: u32` with
/// `image_resize: Option<…>` is behaviour-preserving for every caller that supplies nothing.
#[tokio::test]
async fn read_with_no_profile_keeps_the_2000px_default() {
    assert_eq!(read_png_width(None).await, 2000);
}
