//! `images.autoResize` must reach the `@file` image path (pi `main.ts:828-832`).
//!
//! Pi threads `settingsManager.getImageAutoResize()` into `prepareInitialMessage` (main.ts:830),
//! which hands it to `processFileArguments(parsed.fileArgs, { autoResizeImages })` (main.ts:181) and
//! on to `processImage(content, mimeType, { autoResizeImages })` (cli/file-processor.ts:53). With
//! the flag off, `processImage` returns the NORMALIZED original bytes base64-encoded — no resize, no
//! byte-cap ladder, and no `[Image: original …, displayed at …]` dimension note.
//!
//! cyrup's `build_inputs` took no settings at all, so `cyrup @screenshot.png "what is this"` always
//! downscaled to 2000px and injected the coordinate-remap hint regardless of the setting the
//! settings panel showed as off. This drives `build_inputs` — the exact fn `main.rs` calls at both
//! its one-shot and interactive arms — and asserts on the `Inputs` it returns.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use base64::Engine;
use cyrup::{Cli, Inputs, build_inputs};
use cyrup_sdk::core::Content;

/// Wider than the 2000px edge so the resize branch is unambiguously reachable, and a smooth gradient
/// so the PNG stays far under the 4.5MB base64 cap — the byte ladder must not be what moves here.
const W: u32 = 2600;
const H: u32 = 800;

struct Fx {
    dir: tempfile::TempDir,
    /// The bytes on disk, i.e. exactly what `autoResize: false` must inline verbatim.
    raw: Vec<u8>,
}

fn fixture() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let img: image::ImageBuffer<image::Rgb<u8>, Vec<u8>> =
        image::ImageBuffer::from_fn(W, H, |x, y| image::Rgb([(x % 251) as u8, (y % 241) as u8, 0]));
    let path = dir.path().join("shot.png");
    img.save_with_format(&path, image::ImageFormat::Png).unwrap();
    let raw = std::fs::read(&path).unwrap();
    Fx { dir, raw }
}

async fn inputs_for(fx: &Fx, auto_resize: bool) -> Inputs {
    let cli = Cli {
        positionals: vec!["@shot.png".to_string(), "what is this".to_string()],
        ..Cli::default()
    };
    build_inputs(&cli, fx.dir.path(), auto_resize).await.unwrap()
}

fn only_image(inputs: &Inputs) -> &str {
    assert_eq!(inputs.images.len(), 1, "the image is attached, not omitted");
    match &inputs.images[0] {
        Content::Image { data, .. } => data,
        other => panic!("expected a Content::Image attachment, got {other:?}"),
    }
}

/// THE FIX. `autoResize` off ⇒ the attachment is the file's own bytes and the `<file>` tag carries
/// no dimension note.
///
/// Before the fix `build_inputs` had no such parameter at all; once threaded but left unread this
/// failed with the `[Image: original 2600x800, displayed at 2000x615 …]` note still present.
#[tokio::test]
async fn auto_resize_off_inlines_the_original_bytes() {
    let fx = fixture();
    let inputs = inputs_for(&fx, false).await;

    assert_eq!(
        only_image(&inputs),
        base64::engine::general_purpose::STANDARD.encode(&fx.raw),
        "a PNG is already a supported inline mime, so normalizeImage is the identity and the \
         attachment must be byte-identical to the file on disk (pi image-process.ts final block)"
    );
    assert!(
        !inputs.initial.contains("displayed at"),
        "no resize happened, so `formatDimensionNote` never runs. Got prompt: {}",
        inputs.initial
    );
    // The `<file>` reference is still emitted, just with an empty body (file-processor.ts:67-72).
    assert!(
        inputs.initial.contains("shot.png\"></file>"),
        "got prompt: {}",
        inputs.initial
    );
}

/// The mirror case, so the assertion above cannot pass vacuously: the SAME fixture through the SAME
/// fn with the setting ON is downscaled and annotated, exactly as before this change.
#[tokio::test]
async fn auto_resize_on_still_downscales_and_annotates() {
    let fx = fixture();
    let inputs = inputs_for(&fx, true).await;

    assert_ne!(
        only_image(&inputs),
        base64::engine::general_purpose::STANDARD.encode(&fx.raw),
        "a downscaled image is re-encoded, so it cannot equal the source bytes"
    );
    assert!(
        inputs
            .initial
            .contains(&format!("[Image: original {W}x{H}, displayed at 2000x")),
        "got prompt: {}",
        inputs.initial
    );
}
