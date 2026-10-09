//! `process_image` — the ONE image normalize + resize path (Pi `processImage`,
//! `packages/coding-agent/src/utils/image-process.ts:70-116` @v1.0.4, over
//! `resizeImageInProcess`, `image-resize-core.ts:59-164`).
//!
//! # Why this module exists
//!
//! cyrup carried TWO near-identical ports of this one upstream function, and they disagreed:
//! `crates/cyrup/src/input.rs` (the CLI `@file` path) and a private `mod image_proc` inside
//! `crates/cyrup-tools/src/tools/read.rs` (the `read` tool). They agreed on the 4.5 MiB base64
//! cap, the `[80, 85, 70, 55, 40]` quality ladder, Lanczos3 + exact resize, the PNG-then-JPEG
//! candidate order, the ×0.75 floor-shrink loop with its 1×1 terminator, the already-within-limits
//! early return that re-emits the ORIGINAL bytes, and both `[Image omitted: …]` strings. They
//! differed in four ways that matter:
//!
//! 1. **EXIF orientation.** `read.rs`'s copy decoded through `decode_with_orientation` and baked
//!    the tag into the pixels on both the convert and the resize path; `input.rs`'s used a bare
//!    `image::load_from_memory` and did neither, so a rotated phone screenshot reached the model
//!    sideways. Upstream applies `applyExifOrientation` (`image-resize-core.ts:72`), so `read.rs`
//!    was the faithful one — this module keeps its behaviour and the `@file` path gains it.
//! 2. **The clamp.** Both collapsed upstream's two SEQUENTIAL clamps into one number
//!    (`MAX_IMAGE_EDGE`, `max_image_dim`). `maxWidth` and `maxHeight` are not interchangeable with
//!    a single `max_dim`: a 3000×500 image under `{maxWidth: 1000, maxHeight: 2000}` lands at
//!    1000×167 with the second clamp inert, which one value cannot express.
//! 3. **MIME typing.** `read.rs`'s took the sniffed [`crate::ops::ImageMime`] enum and could not
//!    express an arbitrary wire MIME; `input.rs`'s took `&str` and ported Pi's `baseMimeType` +
//!    `normalizeSupportedImageMimeType` string predicate, which is what an RPC/SDK prompt image
//!    needs (its `mime_type` is a caller-supplied string, not a sniff). This module takes `&str`.
//! 4. **The failure shape.** `input.rs`'s returned `Result<_, ImageOmit>`, a two-variant enum whose
//!    message was a `&'static str` reconstructed at the call site. That cannot carry a per-call
//!    message, and once the profile is configurable the two static strings stop being exhaustive.
//!    `read.rs`'s `Processed::{Ok { data, mime, hints }, Failed { message }}` is already upstream's
//!    `ok` / `message` / `hints` shape, and is what [`Processed`] is.
//!
//! # The profile
//!
//! [`ProcessImageOptions::resize`] is the model's `inputLimits.images.resize`
//! ([`ModelImageResizeOptions`]), read off the request model. It is `Option`, and every key inside
//! it is `Option`, all the way down: each key resolves INDEPENDENTLY against
//! [`cyrup_core::DEFAULT_IMAGE_RESIZE`] (upstream's `{ ...DEFAULT_OPTIONS, ...options }`,
//! `image-resize-core.ts:64`), so a row naming only `maxBytes` keeps the 2000px clamps. `None`
//! resolves to the defaults in full — byte-for-byte the fixed profile both previous copies
//! hardcoded, which is why wiring this module in changes no output until a profile is supplied.

use cyrup_core::{ModelImageResizeOptions, ResolvedResize};
use image::{DynamicImage, ImageDecoder};
use std::borrow::Cow;
use std::io::Cursor;

/// Pi `ProcessImageOptions` (`image-process.ts:4-9`).
#[derive(Clone, Copy, Debug)]
pub struct ProcessImageOptions<'a> {
    /// Pi `options.autoResizeImages ?? true` (`image-process.ts:71`) — the `images.autoResize`
    /// setting. `true` runs `normalizeImage` then the `resizeImage` ladder; `false` normalizes
    /// ONLY and inlines the normalized bytes, with the conversion hint compared against the
    /// NORMALIZED mime (never a re-encoded one) and no dimension note.
    pub auto_resize_images: bool,
    /// Pi `options.resizeOptions` — the request model's `inputLimits.images.resize`. `None` uses
    /// [`cyrup_core::DEFAULT_IMAGE_RESIZE`] for every key.
    pub resize: Option<&'a ModelImageResizeOptions>,
}

impl Default for ProcessImageOptions<'_> {
    /// Upstream's own defaults: `autoResizeImages` defaults to **`true`** (`image-process.ts:71`,
    /// `options?.autoResizeImages ?? true`), and an absent profile resolves to
    /// [`cyrup_core::DEFAULT_IMAGE_RESIZE`].
    fn default() -> Self {
        Self {
            auto_resize_images: true,
            resize: None,
        }
    }
}

/// Pi `ProcessImageResult` (`image-process.ts:11-21`): a discriminated union, not an `Option`.
///
/// The `ok: false` arm carries a MESSAGE, and the `ok: true` arm carries HINTS — a successful
/// resize still produces up to two of them (the conversion hint and the dimension note), which is
/// why neither of the shapes this replaces (`Option<Resized>`, `Result<_, ImageOmit>`) could
/// express it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Processed {
    Ok {
        /// Standard-alphabet base64 of the final bytes.
        data: String,
        /// The final MIME — possibly format-switched by the re-encode ladder.
        mime: String,
        /// `[Image converted from … to ….]` and/or `[Image: original WxH, displayed at WxH. …]`.
        hints: Vec<String>,
    },
    Failed {
        /// One of Pi's two `[Image omitted: …]` strings.
        message: String,
    },
}

/// A settled resize result mirroring Pi `ResizedImage` (`image-resize-core.ts:11-19`).
struct Resized {
    data: String,
    mime: String,
    original_width: u32,
    original_height: u32,
    width: u32,
    height: u32,
    was_resized: bool,
}

/// Pi `baseMimeType` (`image-process.ts:28-30`): strip any `;charset=…` suffix and lowercase.
fn base_mime_type(mime_type: &str) -> String {
    mime_type
        .split(';')
        .next()
        .unwrap_or(mime_type)
        .trim()
        .to_ascii_lowercase()
}

/// Pi `normalizeSupportedImageMimeType` (`image-process.ts:32-46`): the four inline-supported
/// formats keep their own format, `image/jpg` folds to `image/jpeg`, and anything else (notably
/// `image/bmp`) returns `None`, signalling a PNG conversion.
///
/// This is a predicate over the DECLARED string, exactly as upstream's is — it does not sniff the
/// bytes. Both of this module's callers hand it a sniffed [`crate::ops::ImageMime`]'s own
/// `.mime()`, so for them every input lands in one of these five arms; a wire MIME from an
/// RPC/SDK prompt image may not, and then the convert-to-PNG arm is upstream's answer.
fn normalize_supported_image_mime_type(mime_type: &str) -> Option<&'static str> {
    match base_mime_type(mime_type).as_str() {
        "image/png" => Some("image/png"),
        "image/jpeg" | "image/jpg" => Some("image/jpeg"),
        "image/gif" => Some("image/gif"),
        "image/webp" => Some("image/webp"),
        _ => None,
    }
}

/// Pi `conversionHint` (`image-process.ts:65-68`).
fn conversion_hint(from: Option<&str>, to: &str) -> Option<String> {
    let from = from?;
    if from == to {
        return None;
    }
    Some(format!("[Image converted from {from} to {to}.]"))
}

/// Pi `processImage` (`image-process.ts:70-116` @v1.0.4), the single entry point.
///
/// `normalizeImage` first — keep the source format for PNG/JPEG/GIF/WebP, convert anything else to
/// PNG (a decode/encode failure here is the "could not be converted" omission). Then, when
/// [`ProcessImageOptions::auto_resize_images`] is set, [`resize_image`]'s clamp + byte-cap ladder
/// (a `None` there is the "could not be resized" omission); otherwise the normalized bytes are
/// inlined as-is.
pub fn process_image(bytes: &[u8], mime_type: &str, options: ProcessImageOptions<'_>) -> Processed {
    // normalizeImage (image-process.ts:48-63).
    let (norm_bytes, norm_mime, converted_from): (Cow<'_, [u8]>, &str, Option<String>) =
        match normalize_supported_image_mime_type(mime_type) {
            Some(mime) => (Cow::Borrowed(bytes), mime, None),
            None => match convert_to_png(bytes) {
                Some(png) => (
                    Cow::Owned(png),
                    "image/png",
                    Some(base_mime_type(mime_type)),
                ),
                None => {
                    return Processed::Failed {
                        message:
                            "[Image omitted: could not be converted to a supported inline image format.]"
                                .to_string(),
                    };
                }
            },
        };

    // `if (autoResizeImages) { … }` — the false path returns the normalized bytes base64-encoded
    // with no clamp, no byte-cap ladder and no dimension note (image-process.ts:107-115).
    if !options.auto_resize_images {
        let mut hints: Vec<String> = Vec::new();
        if let Some(h) = conversion_hint(converted_from.as_deref(), norm_mime) {
            hints.push(h);
        }
        return Processed::Ok {
            data: base64_encode(&norm_bytes),
            mime: norm_mime.to_string(),
            hints,
        };
    }

    match resize_image(&norm_bytes, norm_mime, options.resize) {
        Some(r) => {
            let mut hints: Vec<String> = Vec::new();
            // conversionHint compared against the FINAL re-encoded mime (image-process.ts:93), so
            // a BMP that converts to PNG and then re-encodes to JPEG reads "from image/bmp to
            // image/jpeg".
            if let Some(h) = conversion_hint(converted_from.as_deref(), &r.mime) {
                hints.push(h);
            }
            // formatDimensionNote (image-resize.ts:116-123): only when a resize occurred.
            if r.was_resized {
                let scale = f64::from(r.original_width) / f64::from(r.width.max(1));
                hints.push(format!(
                    "[Image: original {}x{}, displayed at {}x{}. Multiply coordinates by {scale:.2} \
                     to map to original image.]",
                    r.original_width, r.original_height, r.width, r.height
                ));
            }
            Processed::Ok {
                data: r.data,
                mime: r.mime,
                hints,
            }
        }
        None => Processed::Failed {
            message: "[Image omitted: could not be resized below the inline image size limit.]"
                .to_string(),
        },
    }
}

/// Pi `convertImageBytesToPng` (`image-convert.ts:4-24`): decode (EXIF-oriented) + re-encode PNG.
fn convert_to_png(bytes: &[u8]) -> Option<Vec<u8>> {
    encode_png(&decode_with_orientation(bytes)?)
}

/// Decode + bake EXIF orientation into the pixels (Pi `applyExifOrientation`,
/// `image-resize-core.ts:72` and `image-convert.ts:10`). The `image` crate exposes the decoder's
/// orientation tag, which `apply_orientation` rewrites the pixel buffer for.
fn decode_with_orientation(bytes: &[u8]) -> Option<DynamicImage> {
    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut decoder = reader.into_decoder().ok()?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder).ok()?;
    img.apply_orientation(orientation);
    Some(img)
}

fn encode_png(img: &DynamicImage) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    img.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
        .ok()?;
    Some(buf)
}

fn encode_jpeg(img: &DynamicImage, quality: u8) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    // JPEG has no alpha channel, so flatten to RGB8 first (Photon's encoder likewise drops it).
    let rgb = img.to_rgb8();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality)
        .encode_image(&rgb)
        .ok()?;
    Some(buf)
}

/// Pi `resizeImageInProcess` (`image-resize-core.ts:59-164`).
///
/// `profile` resolves PER KEY against [`cyrup_core::DEFAULT_IMAGE_RESIZE`]. Returns `None` only when the image
/// cannot be brought under the byte budget even at 1×1, or when it does not decode.
fn resize_image(
    bytes: &[u8],
    mime: &str,
    profile: Option<&ModelImageResizeOptions>,
) -> Option<Resized> {
    let opts: ResolvedResize = ModelImageResizeOptions::resolve(profile);
    let max_bytes = usize::try_from(opts.max_bytes).unwrap_or(usize::MAX);

    // Pi `inputBase64Size = Math.ceil(inputBytes.byteLength / 3) * 4` (image-resize-core.ts:65).
    let input_base64_size = bytes.len().div_ceil(3) * 4;
    let img = decode_with_orientation(bytes)?;
    let original_width = img.width();
    let original_height = img.height();

    // Already within all limits (BOTH clamps AND the encoded size) ⇒ send the ORIGINAL
    // (normalized) bytes untouched (image-resize-core.ts:83-93).
    if original_width <= opts.max_width
        && original_height <= opts.max_height
        && input_base64_size < max_bytes
    {
        return Some(Resized {
            data: base64_encode(bytes),
            mime: mime.to_string(),
            original_width,
            original_height,
            width: original_width,
            height: original_height,
            was_resized: false,
        });
    }

    // Initial target dims: TWO SEQUENTIAL clamps, aspect-ratio-preserving, `Math.round`
    // (image-resize-core.ts:96-106). They are applied in this order and are not interchangeable —
    // the width clamp can leave the height clamp inert, and vice versa.
    let (mut target_w, mut target_h) = (original_width, original_height);
    if target_w > opts.max_width {
        target_h = ((f64::from(target_h) * f64::from(opts.max_width)) / f64::from(target_w)).round()
            as u32;
        target_w = opts.max_width;
    }
    if target_h > opts.max_height {
        target_w = ((f64::from(target_w) * f64::from(opts.max_height)) / f64::from(target_h))
            .round() as u32;
        target_h = opts.max_height;
    }

    let ladder = opts.quality_ladder();
    let (mut cw, mut ch) = (target_w.max(1), target_h.max(1));
    loop {
        let resized = img.resize_exact(cw, ch, image::imageops::FilterType::Lanczos3);
        // Candidate order (image-resize-core.ts:108-120): PNG first, then JPEG by quality step.
        if let Some(png) = encode_png(&resized) {
            let data = base64_encode(&png);
            if data.len() < max_bytes {
                return Some(Resized {
                    data,
                    mime: "image/png".to_string(),
                    original_width,
                    original_height,
                    width: cw,
                    height: ch,
                    was_resized: true,
                });
            }
        }
        for q in &ladder {
            if let Some(jpg) = encode_jpeg(&resized, *q) {
                let data = base64_encode(&jpg);
                if data.len() < max_bytes {
                    return Some(Resized {
                        data,
                        mime: "image/jpeg".to_string(),
                        original_width,
                        original_height,
                        width: cw,
                        height: ch,
                        was_resized: true,
                    });
                }
            }
        }

        if cw == 1 && ch == 1 {
            break;
        }
        // image-resize-core.ts:146-153: shrink each axis ×0.75 (floor, min 1); stop when neither
        // axis moves.
        let nw = if cw == 1 {
            1
        } else {
            1.max((f64::from(cw) * 0.75).floor() as u32)
        };
        let nh = if ch == 1 {
            1
        } else {
            1.max((f64::from(ch) * 0.75).floor() as u32)
        };
        if nw == cw && nh == ch {
            break;
        }
        cw = nw;
        ch = nh;
    }
    None
}

/// Minimal RFC 4648 standard base64 (matches Node's `Buffer.toString("base64")`).
///
/// Crate-private: it exists because the resizer's output has to be re-encoded in the same place it
/// is produced, and it is NOT the workspace's base64 — everything else, including this module's own
/// callers in `cyrup-session-svc`, goes through `base64::engine`. Making it public invited a second
/// encoder into the tree with no caller to justify it.
pub(crate) fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = *chunk.first().unwrap_or(&0);
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        let n = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        let push = |out: &mut String, idx: usize| {
            out.push(*TABLE.get(idx & 63).unwrap_or(&b'A') as char);
        };
        push(&mut out, (n >> 18) as usize);
        push(&mut out, (n >> 12) as usize);
        if chunk.len() > 1 {
            push(&mut out, (n >> 6) as usize);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            push(&mut out, n as usize);
        } else {
            out.push('=');
        }
    }
    out
}

/// Standard-alphabet base64 DECODE, for tests that need to measure the real pixels of an output
/// rather than trust a hint string. Test-only: nothing in the resizer decodes base64.
#[cfg(test)]
pub fn base64_decode_for_tests(s: &str) -> Vec<u8> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    let mut out = Vec::new();
    for c in s.bytes().filter(|b| *b != b'=') {
        let Some(v) = TABLE.iter().position(|t| *t == c) else {
            continue;
        };
        acc = (acc << 6) | (v as u32);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}
