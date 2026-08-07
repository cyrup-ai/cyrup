//! Image byte-cap integration test (gap-analysis 13-cyrup §I): drive the assembled `@file` input
//! path (`build_inputs`, the exact fn `main.rs` calls) with a dense sub-2000px image whose PNG base64
//! exceeds Pi's 4.5MB inline cap, and assert the attached `Content::Image` is re-encoded BELOW the cap
//! — i.e. the JPEG-quality re-encode ladder (Pi `resizeImageInProcess`, image-resize-core.ts:59-164)
//! fires on encoded byte size, not just pixel dimensions.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use base64::Engine;
use cyrup::{Cli, build_inputs};
use cyrup_sdk::core::Content;
use image::{ImageBuffer, Rgb};

/// Pi `DEFAULT_MAX_BYTES` (image-resize-core.ts:22): 4.5 · 1024 · 1024 bytes of base64.
const MAX_IMAGE_BASE64_BYTES: usize = 4_718_592;

/// A per-pixel high-entropy value so the generated PNG is (near-)incompressible — a real screenshot
/// this dense would blow the inline cap while staying well under 2000px.
fn pixel_hash(x: u32, y: u32) -> u32 {
    let mut h = x
        .wrapping_mul(374_761_393)
        .wrapping_add(y.wrapping_mul(668_265_263));
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    h ^ (h >> 16)
}

#[tokio::test]
async fn dense_sub_2000px_image_is_reencoded_below_the_byte_cap() {
    let dir = tempfile::tempdir().unwrap();

    // 1500×1500 (< 2000 on every edge, so the OLD dimension-only path would never touch it), filled
    // with white noise so the PNG cannot compress below ~raw size.
    let edge: u32 = 1500;
    let img: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_fn(edge, edge, |x, y| {
        let v = pixel_hash(x, y);
        Rgb([v as u8, (v >> 8) as u8, (v >> 16) as u8])
    });
    let png_path = dir.path().join("big.png");
    img.save_with_format(&png_path, image::ImageFormat::Png)
        .unwrap();

    // Sanity: the SOURCE PNG really is over the base64 cap, so this exercises the byte ladder (not a
    // dimension downscale and not a no-op).
    let raw = std::fs::read(&png_path).unwrap();
    let source_b64 = base64::engine::general_purpose::STANDARD.encode(&raw);
    assert!(
        source_b64.len() > MAX_IMAGE_BASE64_BYTES,
        "test fixture must exceed the cap ({} base64 bytes) to exercise the ladder",
        source_b64.len()
    );

    // Drive the assembled input-assembly path exactly as the bin does.
    let cli = Cli {
        positionals: vec!["@big.png".to_string()],
        ..Cli::default()
    };
    // `true` = the `images.autoResize` default, which is what this byte-cap assertion is about.
    let inputs = build_inputs(&cli, dir.path(), true).await.unwrap();

    assert_eq!(inputs.images.len(), 1, "the image is attached, not omitted");
    let Content::Image { data, mime_type } = &inputs.images[0] else {
        panic!("expected a Content::Image attachment");
    };
    assert!(
        data.len() < MAX_IMAGE_BASE64_BYTES,
        "attached image base64 ({} bytes, mime {mime_type}) must be under Pi's 4.5MB cap; \
         source was {} base64 bytes",
        data.len(),
        source_b64.len()
    );

    // A byte-cap resize occurred, so Pi's dimension note is emitted (formatDimensionNote); the source
    // was already a supported PNG, so there is no "converted from" hint.
    assert!(
        inputs.initial.contains("Multiply coordinates by"),
        "a re-encode/resize emits the dimension note (Pi formatDimensionNote), got: {}",
        inputs.initial
    );
    assert!(
        !inputs.initial.contains("Image converted from"),
        "a supported PNG source must not carry a conversion hint"
    );
}
