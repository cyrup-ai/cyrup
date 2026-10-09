//! The `@file` startup resize is GONE — the session owns it now (SEAM-128, pi `main.ts:221-223`).
//!
//! # What this file used to assert, and why it is inverted
//!
//! It used to pin the OTHER half of the `images.autoResize` wiring: `build_inputs` took the setting
//! and `@screenshot.png` was downsampled to 2000px with a `[Image: original …, displayed at …]`
//! note whenever it was on. Upstream has since moved that work: `prepareInitialMessage` dropped its
//! `autoResizeImages` argument and now calls
//! `processFileArguments(parsed.fileArgs, { autoResizeImages: false })` with the comment
//! *"AgentSession resizes these after extension hooks select the request model"*
//! (`packages/coding-agent/src/main.ts:221-223` @v1.0.4, re-read via `git -C tmp/pi show`).
//!
//! The reason is not tidiness. The CLI boundary runs before a session exists, so a resize applied
//! there is applied against a profile nobody chose — cyrup's was a 2000px / 4.5 MiB / quality-80
//! constant compiled into `input.rs`. `AgentSession::normalize_prompt_images` now resizes every
//! prompt image, from every entry point, against the REQUEST model's `inputLimits.images.resize`,
//! after `before_agent_start` has had its say about which model that is.
//!
//! So the assertions here flip: the `@file` attachment is the file's own bytes and carries no
//! dimension note, and the end-to-end guarantee it used to provide — a 2600px `@file` image does not
//! reach the provider at 2600px — is proved where it now happens, in
//! `cyrup-session-svc/src/tests/prompt_image_normalization.rs`
//! (`a_cli_file_image_still_reaches_the_provider_downscaled`). Deleting this module instead would
//! have dropped the only assertion that the `@file` path stops resizing.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::{Cli, Inputs, build_inputs};
use base64::Engine;
use cyrup_sdk::core::Content;

/// Wider than the 2000px edge, so a surviving startup resize would be unmistakable, and a smooth
/// gradient so the PNG stays far under the 4.5MB base64 cap — the byte ladder must not be what
/// moves here either.
const W: u32 = 2600;
const H: u32 = 800;

struct Fx {
    dir: tempfile::TempDir,
    /// The bytes on disk, i.e. exactly what the `@file` path must now inline verbatim.
    raw: Vec<u8>,
}

fn fixture() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let img: image::ImageBuffer<image::Rgb<u8>, Vec<u8>> =
        image::ImageBuffer::from_fn(W, H, |x, y| {
            image::Rgb([(x % 251) as u8, (y % 241) as u8, 0])
        });
    let path = dir.path().join("shot.png");
    img.save_with_format(&path, image::ImageFormat::Png)
        .unwrap();
    let raw = std::fs::read(&path).unwrap();
    Fx { dir, raw }
}

async fn inputs_for(fx: &Fx) -> Inputs {
    let cli = Cli {
        positionals: vec!["@shot.png".to_string(), "what is this".to_string()],
        ..Cli::default()
    };
    // `None` is Pi's `stdinContent` argument (main.ts:831). Passing it explicitly is what makes
    // this target hermetic: `build_inputs` used to read the process's own stdin, so under
    // `cargo test` it inherited the runner's descriptor and hung forever whenever that was a pipe
    // nobody closed (SEAM-072). The read is now `main.rs`'s, exactly as it is Pi's `main`'s.
    build_inputs(&cli, fx.dir.path(), None).await.unwrap()
}

fn only_image(inputs: &Inputs) -> &str {
    assert_eq!(inputs.images.len(), 1, "the image is attached, not omitted");
    match &inputs.images[0] {
        Content::Image { data, .. } => data,
        other => panic!("expected a Content::Image attachment, got {other:?}"),
    }
}

/// THE DROP. A 2600px `@file` image is attached at full resolution with no dimension note: nothing
/// on this path resizes any more, whatever `images.autoResize` says, because the session does it
/// against the request model's profile.
#[tokio::test]
async fn file_args_inline_the_original_bytes_and_never_resize() {
    let fx = fixture();
    let inputs = inputs_for(&fx).await;

    assert_eq!(
        only_image(&inputs),
        base64::engine::general_purpose::STANDARD.encode(&fx.raw),
        "a PNG is already a supported inline mime, so `normalizeImage` is the identity and the \
         attachment must be byte-identical to the file on disk (pi image-process.ts final block)"
    );
    assert!(
        !inputs.initial.contains("displayed at"),
        "no resize happens at the CLI boundary any more, so `formatDimensionNote` never runs. \
         Got prompt: {}",
        inputs.initial
    );
    // The `<file>` reference is still emitted, just with an empty body (file-processor.ts:67-72) —
    // the tag contract is untouched by the drop.
    assert!(
        inputs.initial.contains("shot.png\"></file>"),
        "got prompt: {}",
        inputs.initial
    );
}

/// The byte cap is not applied here either. This is the case the deleted `image_bytecap` module
/// covered from the other side: a dense sub-2000px PNG whose base64 exceeds pi's 4.5 MiB inline cap
/// used to be re-encoded by `build_inputs`. It now reaches the session untouched, where the ladder
/// runs against the request model's `maxBytes`. Without this assertion the drop could be half-done —
/// dimensions left alone but the byte ladder still firing — and the test above would not notice.
#[tokio::test]
async fn file_args_do_not_apply_the_byte_cap_either() {
    /// Pi `DEFAULT_MAX_BYTES` (image-resize-core.ts:22): 4.5 · 1024 · 1024 bytes of base64.
    const MAX_IMAGE_BASE64_BYTES: usize = 4_718_592;
    /// A per-pixel high-entropy value so the generated PNG is (near-)incompressible.
    fn pixel_hash(x: u32, y: u32) -> u32 {
        let mut h = x
            .wrapping_mul(374_761_393)
            .wrapping_add(y.wrapping_mul(668_265_263));
        h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
        h ^ (h >> 16)
    }

    let dir = tempfile::tempdir().unwrap();
    // 1500×1500 — under 2000 on every edge, so only the BYTE ladder could ever touch it.
    let edge: u32 = 1500;
    let img: image::ImageBuffer<image::Rgb<u8>, Vec<u8>> =
        image::ImageBuffer::from_fn(edge, edge, |x, y| {
            let v = pixel_hash(x, y);
            image::Rgb([v as u8, (v >> 8) as u8, (v >> 16) as u8])
        });
    let path = dir.path().join("big.png");
    img.save_with_format(&path, image::ImageFormat::Png)
        .unwrap();
    let raw = std::fs::read(&path).unwrap();
    let source_b64 = base64::engine::general_purpose::STANDARD.encode(&raw);
    assert!(
        source_b64.len() > MAX_IMAGE_BASE64_BYTES,
        "fixture must exceed the cap ({} base64 bytes) for this assertion to mean anything",
        source_b64.len()
    );

    let cli = Cli {
        positionals: vec!["@big.png".to_string()],
        ..Cli::default()
    };
    let inputs = build_inputs(&cli, dir.path(), None).await.unwrap();

    assert_eq!(inputs.images.len(), 1);
    let Content::Image { data, .. } = &inputs.images[0] else {
        panic!("expected a Content::Image attachment");
    };
    assert_eq!(
        data, &source_b64,
        "the over-cap PNG must reach the session whole; the ladder is the session's to run"
    );
    assert!(
        !inputs.initial.contains("Multiply coordinates by"),
        "no re-encode happened here, so no dimension note. Got: {}",
        inputs.initial
    );
}
