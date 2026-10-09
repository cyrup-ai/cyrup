//! The ONE `process_image` (SEAM-128): the profile really drives it, and the contract is Pi's.
//!
//! These tests exist because cyrup carried TWO ports of Pi's `processImage` that disagreed, and
//! because each one collapsed upstream's four-key profile into hardcoded constants — a shape in
//! which three of the four keys are unreachable and the fourth (`maxWidth`/`maxHeight`) is wrong
//! for any non-square profile. Every assertion here is one upstream behaviour that neither
//! previous copy could produce.
//!
//! Upstream re-read at v1.0.4 via `git -C tmp/pi show`:
//! `packages/coding-agent/src/utils/image-process.ts:70-116` (`processImage`),
//! `.../image-resize-core.ts:4-9` (the four keys), `:24-29` (`DEFAULT_OPTIONS`),
//! `:64` (`{ ...DEFAULT_OPTIONS, ...options }`), `:72` (`applyExifOrientation`),
//! `:96-106` (the two SEQUENTIAL clamps), `:121` (the quality ladder),
//! `.../image-resize.ts:116-123` (`formatDimensionNote`).
//!
//! No image binaries are committed: every fixture is built with the `image` crate here, following
//! `crates/cyrup/src/tests/image_auto_resize_file_args.rs` and `image_bytecap.rs`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::image_proc::{ProcessImageOptions, Processed, process_image};
use cyrup_core::{DEFAULT_IMAGE_RESIZE, ModelImageResizeOptions};

/// Pseudo-random RGB NOISE of the given size. Deliberately incompressible rather than flat or
/// gradient: a smooth image compresses so well that a byte-cap assertion is satisfied at the
/// original dimensions and proves nothing, and PNG would win every candidate race so the JPEG
/// ladder would never be exercised. The generator is a fixed-seed xorshift, so the fixtures are
/// identical on every run and on every platform.
fn noise(w: u32, h: u32) -> image::RgbImage {
    let mut state: u32 = 0x9E37_79B9;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    image::ImageBuffer::from_fn(w, h, |_, _| {
        let n = next();
        image::Rgb([n as u8, (n >> 8) as u8, (n >> 16) as u8])
    })
}

fn encode(img: &image::RgbImage, fmt: image::ImageFormat) -> Vec<u8> {
    let mut out = Vec::new();
    image::DynamicImage::ImageRgb8(img.clone())
        .write_to(&mut std::io::Cursor::new(&mut out), fmt)
        .unwrap();
    out
}

fn noisy_png(w: u32, h: u32) -> Vec<u8> {
    encode(&noise(w, h), image::ImageFormat::Png)
}

fn bmp(w: u32, h: u32) -> Vec<u8> {
    encode(&noise(w, h), image::ImageFormat::Bmp)
}

fn b64_decode(s: &str) -> Vec<u8> {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    let mut out = Vec::new();
    for c in s.bytes().filter(|b| *b != b'=') {
        let v = T.iter().position(|t| *t == c).expect("base64 alphabet") as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

/// `(width, height)` of a base64 payload, decoded for real rather than inferred from a hint.
fn dims(data: &str) -> (u32, u32) {
    let bytes = b64_decode(data);
    let img = image::load_from_memory(&bytes).expect("decodable output");
    (img.width(), img.height())
}

fn ok(p: Processed) -> (String, String, Vec<String>) {
    match p {
        Processed::Ok { data, mime, hints } => (data, mime, hints),
        Processed::Failed { message } => panic!("expected Ok, got Failed: {message}"),
    }
}

fn profile(
    max_width: Option<u32>,
    max_height: Option<u32>,
    max_bytes: Option<u64>,
    jpeg_quality: Option<u8>,
) -> ModelImageResizeOptions {
    ModelImageResizeOptions {
        max_width,
        max_height,
        max_bytes,
        jpeg_quality,
    }
}

fn run(bytes: &[u8], mime: &str, p: Option<&ModelImageResizeOptions>) -> Processed {
    process_image(
        bytes,
        mime,
        ProcessImageOptions {
            auto_resize_images: true,
            resize: p,
        },
    )
}

// ---------------------------------------------------------------------------------------------
// 1. `maxWidth` and `maxHeight` are TWO SEQUENTIAL clamps, not one `max_dim`.
// ---------------------------------------------------------------------------------------------

/// `image-resize-core.ts:96-106`: the width clamp runs first and can leave the height clamp inert.
/// A collapsed single-`max_dim` port cannot express this profile at all, and a port that applies
/// `min(maxWidth, maxHeight)` to both axes gets the SECOND case wrong — which is why both
/// orientations are asserted.
#[test]
fn a_non_square_profile_applies_both_clamps_in_order() {
    let wide = profile(Some(1000), Some(2000), None, None);

    // 3000x500: width clamp binds (→ 1000x167), height clamp then inert (167 <= 2000).
    let (data, _, _) = ok(run(&noisy_png(3000, 500), "image/png", Some(&wide)));
    assert_eq!(dims(&data), (1000, 167), "width clamp must bind alone");

    // 500x3000, SAME profile: width clamp inert (500 <= 1000), height clamp binds (→ 333x2000).
    let (data, _, _) = ok(run(&noisy_png(500, 3000), "image/png", Some(&wide)));
    assert_eq!(dims(&data), (333, 2000), "height clamp must bind alone");
}

/// The mirror profile, to pin the ORDER rather than just the pair of numbers: under
/// `{maxWidth: 2000, maxHeight: 1000}` the same two inputs swap which clamp binds.
#[test]
fn the_clamps_are_not_interchangeable() {
    let tall = profile(Some(2000), Some(1000), None, None);
    // 3000x500: only the width clamp binds, and it yields 333 — NOT the 167 the same input
    // yields under `{1000, 2000}` above. The two profiles are distinguishable, which is exactly
    // what a single `max_dim` erases.
    let (data, _, _) = ok(run(&noisy_png(3000, 500), "image/png", Some(&tall)));
    assert_eq!(dims(&data), (2000, 333));
    // 500x3000: only the height clamp binds.
    let (data, _, _) = ok(run(&noisy_png(500, 3000), "image/png", Some(&tall)));
    assert_eq!(dims(&data), (167, 1000));
}

// ---------------------------------------------------------------------------------------------
// 2. `jpegQuality` LEADS the ladder and is de-duplicated.
// ---------------------------------------------------------------------------------------------

/// `image-resize-core.ts:121`: `Array.from(new Set([opts.jpegQuality, 85, 70, 55, 40]))`. The
/// profile's quality leads rather than replacing, and a quality already in the tail collapses the
/// ladder to four steps. Both previous copies hardcoded the five-element literal `[80,85,70,55,40]`,
/// which is only correct for the default.
#[test]
fn the_profiles_jpeg_quality_leads_the_ladder_and_dedups() {
    let ladder = |q: Option<u8>| {
        ModelImageResizeOptions::resolve(Some(&profile(None, None, None, q))).quality_ladder()
    };
    assert_eq!(
        ladder(None),
        vec![80, 85, 70, 55, 40],
        "default leads with 80"
    );
    assert_eq!(ladder(Some(80)), vec![80, 85, 70, 55, 40]);
    assert_eq!(
        ladder(Some(40)),
        vec![40, 85, 70, 55],
        "40 dedups to four steps"
    );
    assert_eq!(ladder(Some(55)), vec![55, 85, 70, 40]);
    assert_eq!(
        ladder(Some(30)),
        vec![30, 85, 70, 55, 40],
        "a novel quality leads five"
    );
}

/// And the lead is OBSERVABLE, not just computed: with the byte cap set so JPEG is reached but the
/// leading quality already fits, the leading quality is the one that is emitted — so a profile
/// asking for 40 produces a strictly smaller payload than one asking for 85 at identical
/// dimensions. A port that iterated the fixed literal would emit quality 80 for both.
#[test]
fn the_leading_jpeg_quality_is_the_one_emitted() {
    let src = noisy_png(200, 200);
    // 200x200 of incompressible noise: its PNG is ~160 KB of base64, so a 60 KB cap rules PNG out
    // at the original dimensions while JPEG at ANY of these qualities fits — which pins the
    // comparison to the quality alone, with no shrink step in between.
    let cap = Some(60_000u64);
    let (lo, lo_mime, _) = ok(run(
        &src,
        "image/png",
        Some(&profile(None, None, cap, Some(40))),
    ));
    let (hi, hi_mime, _) = ok(run(
        &src,
        "image/png",
        Some(&profile(None, None, cap, Some(85))),
    ));
    assert_eq!(
        lo_mime, "image/jpeg",
        "the byte cap must force the JPEG ladder"
    );
    assert_eq!(hi_mime, "image/jpeg");
    assert_eq!(
        dims(&lo),
        dims(&hi),
        "same dimensions, so only quality differs"
    );
    assert!(
        lo.len() < hi.len(),
        "quality 40 must lead its ladder and emit less than quality 85: {} vs {}",
        lo.len(),
        hi.len()
    );
}

// ---------------------------------------------------------------------------------------------
// 3. `maxBytes` is honoured independently of the clamps.
// ---------------------------------------------------------------------------------------------

/// The case a dimension-only port cannot see: an image ALREADY inside both clamps, failed only by
/// the byte cap. `image-resize-core.ts:83-93` requires all three conditions for the untouched
/// early return, so this must come back re-encoded and under the cap. Neither previous copy read
/// `maxBytes` from a profile — both hardcoded 4 718 592.
#[test]
fn max_bytes_alone_shrinks_a_dimension_legal_image() {
    let src = noisy_png(300, 300);
    let cap = 8_000u64;
    let (data, _, hints) = ok(run(
        &src,
        "image/png",
        Some(&profile(None, None, Some(cap), None)),
    ));
    assert!(
        (data.len() as u64) < cap,
        "payload must be under the profile's maxBytes, got {}",
        data.len()
    );
    assert!(
        hints
            .iter()
            .any(|h| h.starts_with("[Image: original 300x300, displayed at ")),
        "a resize under the byte cap still emits the dimension note: {hints:?}"
    );
}

/// And a profile that names ONLY `maxBytes` keeps the 2000px clamps rather than zeroing them —
/// upstream resolves each key independently (`{ ...DEFAULT_OPTIONS, ...options }`,
/// `image-resize-core.ts:64`), which is why every field stays `Option` down to the resizer.
#[test]
fn a_partial_profile_inherits_the_other_keys_per_key() {
    let r = ModelImageResizeOptions::resolve(Some(&profile(None, None, Some(1_234), None)));
    assert_eq!(r.max_bytes, 1_234);
    assert_eq!(
        r.max_width, 2000,
        "maxWidth must survive a maxBytes-only profile"
    );
    assert_eq!(r.max_height, 2000);
    assert_eq!(r.jpeg_quality, 80);

    // An absent profile resolves to the defaults in full — the fixed numbers both deleted copies
    // hardcoded, which is what makes wiring the shared module in a no-op until a profile arrives.
    let d = ModelImageResizeOptions::resolve(None);
    assert_eq!(d.max_width, DEFAULT_IMAGE_RESIZE.max_width.unwrap());
    assert_eq!(d.max_height, DEFAULT_IMAGE_RESIZE.max_height.unwrap());
    assert_eq!(d.max_bytes, DEFAULT_IMAGE_RESIZE.max_bytes.unwrap());
    assert_eq!(d.jpeg_quality, DEFAULT_IMAGE_RESIZE.jpeg_quality.unwrap());
    assert_eq!(
        (d.max_width, d.max_height, d.max_bytes, d.jpeg_quality),
        (2000, 2000, 4_718_592, 80)
    );
}

// ---------------------------------------------------------------------------------------------
// 4. EXIF orientation is baked in (the behavioural divergence the de-duplication resolved).
// ---------------------------------------------------------------------------------------------

/// A JPEG carrying `Orientation = 6` (rotate 90° CW) must come back with its axes swapped:
/// `image-resize-core.ts:72` applies `applyExifOrientation` before measuring or resizing. The
/// `read`-tool copy did this; the `@file` copy used a bare `image::load_from_memory` and did not,
/// so a rotated phone screenshot reached the model sideways. Keeping the faithful one is the whole
/// point of lifting `read`'s copy rather than the binary's.
#[test]
fn exif_orientation_is_applied() {
    let src = jpeg_with_orientation(120, 40, 6);
    // Sanity: a decoder that ignores the tag sees the stored 120x40.
    assert_eq!(
        {
            let i = image::load_from_memory(&src).unwrap();
            (i.width(), i.height())
        },
        (120, 40),
        "the fixture must store 120x40 so the swap below is attributable to the tag"
    );
    // auto_resize off: no clamp can be blamed for the swap, only the orientation.
    let (data, mime, _) = ok(process_image(
        &src,
        "image/jpeg",
        ProcessImageOptions {
            auto_resize_images: true,
            resize: Some(&profile(Some(4000), Some(4000), None, None)),
        },
    ));
    assert_eq!(mime, "image/jpeg");
    assert_eq!(
        dims(&data),
        (120, 40),
        "within limits ⇒ ORIGINAL bytes re-emitted untouched"
    );

    // Force the resize path, which decodes and re-encodes: now the baked orientation shows.
    let (data, _, _) = ok(run(
        &src,
        "image/jpeg",
        Some(&profile(Some(30), Some(30), None, None)),
    ));
    let (w, h) = dims(&data);
    assert!(h > w, "orientation 6 swaps the axes: got {w}x{h}");
    assert_eq!((w, h), (10, 30));
}

/// A baseline JPEG with a minimal EXIF APP1 segment carrying only the Orientation tag. Built here
/// rather than committed: `image` does not write EXIF, so the segment is spliced after SOI.
fn jpeg_with_orientation(w: u32, h: u32, orientation: u16) -> Vec<u8> {
    let img = image::ImageBuffer::from_fn(w, h, |x, y| {
        image::Rgb([(x % 255) as u8, (y % 255) as u8, 64u8])
    });
    let mut jpg = Vec::new();
    image::DynamicImage::ImageRgb8(img)
        .write_to(
            &mut std::io::Cursor::new(&mut jpg),
            image::ImageFormat::Jpeg,
        )
        .unwrap();

    // TIFF (little-endian) header + one IFD entry: 0x0112 Orientation, SHORT, count 1.
    let mut tiff: Vec<u8> = Vec::new();
    tiff.extend_from_slice(b"II*\0");
    tiff.extend_from_slice(&8u32.to_le_bytes()); // offset of IFD0
    tiff.extend_from_slice(&1u16.to_le_bytes()); // entry count
    tiff.extend_from_slice(&0x0112u16.to_le_bytes());
    tiff.extend_from_slice(&3u16.to_le_bytes()); // SHORT
    tiff.extend_from_slice(&1u32.to_le_bytes()); // count
    tiff.extend_from_slice(&orientation.to_le_bytes());
    tiff.extend_from_slice(&[0, 0]); // pad the 4-byte value field
    tiff.extend_from_slice(&0u32.to_le_bytes()); // next IFD = none

    let mut payload: Vec<u8> = b"Exif\0\0".to_vec();
    payload.extend_from_slice(&tiff);
    let seg_len = u16::try_from(payload.len() + 2).unwrap();

    let mut out: Vec<u8> = vec![0xFF, 0xD8, 0xFF, 0xE1];
    out.extend_from_slice(&seg_len.to_be_bytes());
    out.extend_from_slice(&payload);
    out.extend_from_slice(&jpg[2..]); // everything after the source SOI
    out
}

// ---------------------------------------------------------------------------------------------
// 5. The default profile reproduces the behaviour of the constants it replaces.
// ---------------------------------------------------------------------------------------------

/// The guard that makes this lift safely reviewable: with NO profile supplied, the shared module
/// must produce exactly what the deleted `MAX_IMAGE_EDGE = 2000` / `MAX_IMAGE_BASE64_BYTES` /
/// `JPEG_QUALITY_STEPS` constants produced — same dimensions, same `formatDimensionNote` string.
/// 2600x800 is the fixture size `crates/cyrup/src/tests/image_auto_resize_file_args.rs` already
/// asserts against, so the expected hint here is that test's expected hint.
#[test]
fn no_profile_reproduces_the_old_fixed_behaviour() {
    let (data, mime, hints) = ok(run(&noisy_png(2600, 800), "image/png", None));
    assert_eq!(dims(&data), (2000, 615));
    assert!(mime == "image/png" || mime == "image/jpeg");
    assert_eq!(
        hints,
        vec![
            "[Image: original 2600x800, displayed at 2000x615. Multiply coordinates by 1.30 to map \
             to original image.]"
                .to_string()
        ]
    );
}

/// An image inside every limit is re-emitted as the ORIGINAL bytes with `wasResized: false`, hence
/// NO dimension note (`image-resize-core.ts:83-93`, `image-resize.ts:116-123`).
#[test]
fn an_image_within_limits_is_untouched_and_unhinted() {
    let src = noisy_png(64, 64);
    let (data, mime, hints) = ok(run(&src, "image/png", None));
    assert_eq!(mime, "image/png");
    assert!(hints.is_empty(), "no resize ⇒ no hints: {hints:?}");
    assert_eq!(b64_decode(&data), src, "the original bytes, byte for byte");
}

// ---------------------------------------------------------------------------------------------
// 6. The return shape carries a MESSAGE and HINTS — what `Option<Resized>` could not.
// ---------------------------------------------------------------------------------------------

/// Pi's `ok: false` arm (`image-process.ts:78-83`): `normalizeImage` returning null is a MESSAGE,
/// not an absence. This is the corrupt-image case the prompt path turns into a text hint.
#[test]
fn undecodable_bytes_fail_with_the_conversion_message() {
    let mut garbage = b"\x89PNG\r\n\x1a\n".to_vec();
    garbage.extend_from_slice(&[0u8; 64]);
    // Declared mime is deliberately NOT one of the four supported, so normalizeImage must convert.
    match process_image(&garbage, "image/bmp", ProcessImageOptions::default()) {
        Processed::Failed { message } => assert_eq!(
            message,
            "[Image omitted: could not be converted to a supported inline image format.]"
        ),
        Processed::Ok { mime, .. } => panic!("expected Failed, got Ok({mime})"),
    }
}

/// Pi's other `ok: false` arm (`image-process.ts:84-89`): a byte cap no encoding can meet even at
/// 1x1 is the "could not be resized" message. The two messages are distinct and both reachable,
/// which is why a two-variant `ImageOmit` enum was adequate only while the profile was fixed.
#[test]
fn an_impossible_byte_cap_fails_with_the_resize_message() {
    match run(
        &noisy_png(64, 64),
        "image/png",
        Some(&profile(None, None, Some(1), None)),
    ) {
        Processed::Failed { message } => assert_eq!(
            message,
            "[Image omitted: could not be resized below the inline image size limit.]"
        ),
        Processed::Ok { data, .. } => panic!("expected Failed, got {} bytes", data.len()),
    }
}

/// A SUCCESSFUL call still carries hints — up to two of them. `Option<Resized>` had nowhere to put
/// these, and `read`'s old signature could not return them alongside a failure message.
#[test]
fn a_successful_conversion_hints_against_the_final_mime() {
    // BMP → PNG, inside every limit: the conversion hint alone, compared against the NORMALIZED
    // mime on the no-resize branch (`image-process.ts:109-112`).
    let (_, mime, hints) = ok(process_image(
        &bmp(40, 40),
        "image/bmp",
        ProcessImageOptions {
            auto_resize_images: false,
            resize: None,
        },
    ));
    assert_eq!(mime, "image/png");
    assert_eq!(
        hints,
        vec!["[Image converted from image/bmp to image/png.]".to_string()]
    );

    // BMP → PNG that must then re-encode to JPEG to meet the cap: the hint names the FINAL mime
    // (`image-process.ts:93`), and the dimension note joins it.
    // 600x600 of noise capped at 60 KB: PNG is ruled out at every dimension step (noise costs
    // roughly 4 bytes of base64 per pixel as PNG against well under 1 as JPEG), so the ladder
    // settles on JPEG and the conversion hint must name image/jpeg, NOT the image/png that
    // `normalizeImage` produced.
    let (_, mime, hints) = ok(run(
        &bmp(600, 600),
        "image/bmp",
        Some(&profile(None, None, Some(60_000), None)),
    ));
    assert_eq!(mime, "image/jpeg");
    assert_eq!(
        hints.len(),
        2,
        "conversion hint + dimension note: {hints:?}"
    );
    assert_eq!(hints[0], "[Image converted from image/bmp to image/jpeg.]");
    assert!(hints[1].starts_with("[Image: original 600x600, displayed at "));
}

// ---------------------------------------------------------------------------------------------
// 7. The MIME argument is a wire STRING, per Pi's own predicate.
// ---------------------------------------------------------------------------------------------

/// `baseMimeType` + `normalizeSupportedImageMimeType` (`image-process.ts:28-46`): `image/jpg`
/// folds to `image/jpeg` and a `;charset=…` suffix is stripped, so neither is mistaken for an
/// unsupported format and needlessly transcoded to PNG. `read`'s old `ImageMime`-enum signature
/// could not express either spelling — and these are exactly the spellings an RPC/SDK prompt image
/// arrives with, since its `mime_type` is caller-supplied rather than sniffed.
#[test]
fn a_wire_mime_string_is_normalized_not_converted() {
    for (declared, expected) in [
        ("image/jpg", "image/jpeg"),
        ("IMAGE/JPEG", "image/jpeg"),
        ("image/png; charset=binary", "image/png"),
        ("  image/png  ", "image/png"),
    ] {
        let src = if expected == "image/jpeg" {
            let mut v = Vec::new();
            image::DynamicImage::ImageRgb8(image::ImageBuffer::from_fn(32, 32, |x, _| {
                image::Rgb([(x * 8) as u8, 0, 0])
            }))
            .write_to(&mut std::io::Cursor::new(&mut v), image::ImageFormat::Jpeg)
            .unwrap();
            v
        } else {
            noisy_png(32, 32)
        };
        let (data, mime, hints) = ok(run(&src, declared, None));
        assert_eq!(mime, expected, "declared {declared:?}");
        assert!(
            hints.is_empty(),
            "no conversion ⇒ no hint for {declared:?}: {hints:?}"
        );
        assert_eq!(
            b64_decode(&data),
            src,
            "{declared:?} must pass through untranscoded"
        );
    }
}
