//! Prompt-input assembly: positionals + `@file` + piped stdin (arch-11 §6.2; R-11-006/024/025).
//!
//! A 1:1 port of Pi `cli/file-processor.ts` + `cli/initial-message.ts`: `@`-prefixed positionals are
//! file references. Each text file is wrapped `<file name="ABS">\n{content}\n</file>\n`
//! (file-processor.ts:77); each image file is MIME-sniffed, downscaled to fit 2000×2000 AND re-encoded
//! below the 4.5MB base64 cap (Pi `resizeImage`) WHEN the effective `images.autoResize` setting is on
//! (Pi threads `settingsManager.getImageAutoResize()` main.ts:830 → file-processor.ts:53; when off,
//! the normalized original bytes are inlined verbatim), attached as a base64 `Content::Image`, and
//! referenced with an empty `<file name="ABS"></file>\n` tag
//! (file-processor.ts:48-72). Empty files are skipped (file-processor.ts:43); a missing file is a
//! hard error the bin maps to exit 1 (file-processor.ts:37). The initial message is
//! `stdin ⧺ fileText ⧺ messages[0]` joined with `""` (initial-message.ts:27-40) — the file wrapper
//! already supplies its own newlines, so no separators are added.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use base64::Engine;
use cyrup_sdk::core::Content;
use tokio::io::AsyncReadExt;

use crate::cli::Cli;

/// Max image edge before downscale (file-processor.ts `processImage` 2000×2000, image-process.ts).
const MAX_IMAGE_EDGE: u32 = 2000;

/// The assembled prompt inputs for a one-shot / interactive launch.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inputs {
    /// The first prompt: piped stdin ⧺ `@file` text ⧺ first message, joined with `""` (Pi).
    pub initial: String,
    /// `Content::Image` attachments parsed from image `@file` args (attached to the initial message).
    pub images: Vec<Content>,
    /// Subsequent bare messages, replayed one prompt at a time after the initial run (R-11-009).
    pub follow_ups: Vec<String>,
}

impl Inputs {
    /// Whether there is any initial prompt text or image at all.
    pub fn is_empty(&self) -> bool {
        self.initial.is_empty() && self.images.is_empty()
    }
}

/// Split trailing positionals into `@file` references (the `@` stripped) and bare message words.
pub fn split_positionals(positionals: &[String]) -> (Vec<String>, Vec<String>) {
    let mut files = Vec::new();
    let mut messages = Vec::new();
    for arg in positionals {
        match arg.strip_prefix('@') {
            // `@@literal` is an escape for a bare message that legitimately starts with '@'.
            Some(rest) if arg.starts_with("@@") => messages.push(rest.to_string()),
            Some(path) if !path.is_empty() => files.push(path.to_string()),
            _ => messages.push(arg.clone()),
        }
    }
    (files, messages)
}

/// Narrow no-break space — macOS screenshot filenames place it before `AM`/`PM` (Pi
/// `NARROW_NO_BREAK_SPACE`, path-utils.ts:5).
const NARROW_NO_BREAK_SPACE: char = '\u{202F}';

/// Resolve a `@file` spec to an absolute path (Pi `resolve(resolveReadPath(arg, cwd))`,
/// file-processor.ts:31): expand a leading `~`, then make absolute relative to `cwd`. Symlinks are
/// NOT resolved (Pi's `resolve()` is purely lexical). When the literal path does not exist, Pi tries
/// the macOS screenshot variants (path-utils.ts:52-83): a narrow-no-break-space before `AM`/`PM`,
/// NFD-decomposed unicode, and a curly-quote substitution — returning the first variant that exists.
fn resolve_read_path(spec: &str, cwd: &Path) -> PathBuf {
    let expanded = if let Some(rest) = spec.strip_prefix("~/") {
        if let Some(home) = home_dir() {
            home.join(rest)
        } else {
            PathBuf::from(spec)
        }
    } else {
        PathBuf::from(spec)
    };
    let resolved = if expanded.is_absolute() {
        expanded
    } else {
        cwd.join(expanded)
    };
    if resolved.exists() {
        return resolved;
    }
    let original = resolved.to_string_lossy().into_owned();
    // 1) macOS AM/PM narrow-no-break-space variant.
    let am_pm = macos_screenshot_variant(&original);
    if am_pm != original && Path::new(&am_pm).exists() {
        return PathBuf::from(am_pm);
    }
    // 2) NFD-decomposed variant (macOS stores filenames decomposed).
    let nfd = nfd_variant(&original);
    if nfd != original && Path::new(&nfd).exists() {
        return PathBuf::from(&nfd);
    }
    // 3) Curly-quote variant (U+2019 in place of the straight apostrophe).
    let curly = curly_quote_variant(&original);
    if curly != original && Path::new(&curly).exists() {
        return PathBuf::from(curly);
    }
    // 4) Combined NFD + curly quote (French macOS screenshots like "Capture d'écran").
    let nfd_curly = curly_quote_variant(&nfd);
    if nfd_curly != original && Path::new(&nfd_curly).exists() {
        return PathBuf::from(nfd_curly);
    }
    resolved
}

/// Replace a regular space before `AM`/`PM` (case-insensitive) with a narrow no-break space
/// (Pi `tryMacOSScreenshotPath`, path-utils.ts:7-9 — `/ (AM|PM)\./gi`).
fn macos_screenshot_variant(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut rest = path;
    while let Some(space_idx) = rest.find(' ') {
        let (before, after_space) = rest.split_at(space_idx);
        out.push_str(before);
        // `after_space` starts at the space; the candidate "AM."/"PM." follows it.
        let candidate = after_space.get(1..4).unwrap_or("");
        let lower = candidate.to_ascii_lowercase();
        if lower == "am." || lower == "pm." {
            out.push(NARROW_NO_BREAK_SPACE);
        } else {
            out.push(' ');
        }
        rest = after_space.get(1..).unwrap_or("");
    }
    out.push_str(rest);
    out
}

/// NFD-normalize a path (Pi `tryNFDVariant`, path-utils.ts:12-14).
fn nfd_variant(path: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    path.nfd().collect()
}

/// Replace straight apostrophes with U+2019 (Pi `tryCurlyQuoteVariant`, path-utils.ts:17-19).
fn curly_quote_variant(path: &str) -> String {
    path.replace('\'', "\u{2019}")
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// The processed `@file` payload: wrapped text + image attachments (Pi `ProcessedFiles`).
#[derive(Default, Debug)]
struct ProcessedFiles {
    text: String,
    images: Vec<Content>,
}

/// Sniff a supported image MIME type from the leading magic bytes (Pi
/// `detectSupportedImageMimeTypeFromFile`, utils/mime.ts). Returns `None` for non-image content.
fn detect_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.get(0..4) == Some(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else if bytes.starts_with(b"BM") {
        Some("image/bmp")
    } else {
        None
    }
}

/// A processed-image result mirroring Pi `ProcessImageResult` (image-process.ts:11-20): the base64
/// data, the (preserved-or-converted) MIME type, and the processing hint lines.
struct ProcessedImage {
    data: String,
    mime_type: String,
    hints: Vec<String>,
}

/// Map a detected MIME type onto the supported inline image MIME, keeping the original format (Pi
/// `normalizeSupportedImageMimeType`, image-process.ts:33-46). `image/jpg` folds to `image/jpeg`.
/// Anything else (e.g. `image/bmp`) returns `None`, signalling a PNG conversion.
fn supported_inline_mime(mime: &str) -> Option<&'static str> {
    match mime
        .split(';')
        .next()
        .unwrap_or(mime)
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "image/png" => Some("image/png"),
        "image/jpeg" | "image/jpg" => Some("image/jpeg"),
        "image/gif" => Some("image/gif"),
        "image/webp" => Some("image/webp"),
        _ => None,
    }
}

/// The 4.5MB base64-payload cap Pi enforces on inline images (Pi `DEFAULT_MAX_BYTES`,
/// image-resize-core.ts:22): `4.5 · 1024 · 1024` bytes of base64, headroom below Anthropic's 5MB limit.
const MAX_IMAGE_BASE64_BYTES: usize = 4_718_592;

/// The JPEG quality ladder tried at each dimension step (Pi `qualitySteps`, image-resize-core.ts:122 —
/// `Array.from(new Set([jpegQuality=80, 85, 70, 55, 40]))`).
const JPEG_QUALITY_STEPS: [u8; 5] = [80, 85, 70, 55, 40];

/// Why an image could not be inlined, mapping to Pi's two distinct `[Image omitted: …]` messages
/// (image-process.ts:80-92): a normalization failure vs a resize-below-cap failure. The caller renders
/// the matching placeholder into the `<file>` tag (Pi `processed.ok === false`, file-processor.ts).
enum ImageOmit {
    /// Pi `normalizeImage` returned null — an unsupported format that could not be converted to PNG.
    Convert,
    /// Pi `resizeImage` returned null — could not be re-encoded/downscaled below `maxBytes`.
    Resize,
}

impl ImageOmit {
    fn message(&self) -> &'static str {
        match self {
            ImageOmit::Convert => {
                "[Image omitted: could not be converted to a supported inline image format.]"
            }
            ImageOmit::Resize => {
                "[Image omitted: could not be resized below the inline image size limit.]"
            }
        }
    }
}

/// A settled resize result mirroring Pi `ResizedImage` (image-resize-core.ts:11-19): the base64 data,
/// its (possibly format-switched) MIME, the original + displayed dimensions, and whether any re-encode
/// or downscale occurred (drives `formatDimensionNote`).
struct Resized {
    data: String,
    mime_type: String,
    original_width: u32,
    original_height: u32,
    width: u32,
    height: u32,
    was_resized: bool,
}

/// The first base64-encoded candidate under the byte cap at a fixed dimension, tried in Pi's exact
/// preference order (Pi `tryEncodings` candidate array, image-resize-core.ts:108-120): PNG first
/// (lossless when it fits), then JPEG at each quality step. Returns `None` when nothing fits at these
/// dimensions, so the caller shrinks and retries.
fn first_candidate_under_cap(img: &image::DynamicImage) -> Option<(String, &'static str)> {
    use image::ImageEncoder;

    // PNG candidate (Pi `encodeCandidate(resized.get_bytes(), "image/png")`).
    let mut png = Vec::new();
    if img
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .is_ok()
    {
        let data = base64::engine::general_purpose::STANDARD.encode(&png);
        if data.len() < MAX_IMAGE_BASE64_BYTES {
            return Some((data, "image/png"));
        }
    }

    // JPEG candidates at each quality step (Pi `resized.get_bytes_jpeg(quality)`); JPEG has no alpha
    // channel, so flatten to RGB8 first (Photon's JPEG encoder likewise drops alpha).
    let rgb = img.to_rgb8();
    for quality in JPEG_QUALITY_STEPS {
        let mut jpeg = Vec::new();
        if image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, quality)
            .write_image(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                image::ExtendedColorType::Rgb8,
            )
            .is_ok()
        {
            let data = base64::engine::general_purpose::STANDARD.encode(&jpeg);
            if data.len() < MAX_IMAGE_BASE64_BYTES {
                return Some((data, "image/jpeg"));
            }
        }
    }
    None
}

/// Resize an image to fit BOTH `MAX_IMAGE_EDGE` and the base64 byte cap — a 1:1 port of Pi
/// `resizeImageInProcess` (image-resize-core.ts:59-164). If the input is already within the pixel
/// bounds AND under the byte cap it is returned as-is (`was_resized = false`). Otherwise it is scaled
/// to fit the pixel bounds and, at each dimension step, encoded PNG-first then JPEG down the quality
/// ladder; the first candidate under the cap wins. When nothing fits, dimensions shrink by ×0.75 (min
/// 1×1) and the ladder repeats. Returns `None` only when the image cannot be brought under the cap (Pi
/// returns null → the caller emits the "could not be resized" placeholder).
fn resize_image(input_bytes: &[u8], mime_type: &str) -> Option<Resized> {
    // Pi `inputBase64Size = Math.ceil(inputBytes.byteLength / 3) * 4` (image-resize-core.ts:65).
    let input_base64_size = input_bytes.len().div_ceil(3) * 4;

    let decoded = image::load_from_memory(input_bytes).ok()?;
    let original_width = decoded.width();
    let original_height = decoded.height();

    // Already within all limits (dimensions AND encoded size) → return untouched (Pi lines 83-93).
    if original_width <= MAX_IMAGE_EDGE
        && original_height <= MAX_IMAGE_EDGE
        && input_base64_size < MAX_IMAGE_BASE64_BYTES
    {
        return Some(Resized {
            data: base64::engine::general_purpose::STANDARD.encode(input_bytes),
            mime_type: mime_type.to_string(),
            original_width,
            original_height,
            width: original_width,
            height: original_height,
            was_resized: false,
        });
    }

    // Initial target dimensions respecting the max edge, aspect-ratio-preserving (Pi lines 96-106,
    // `Math.round`). `resize_exact` below then reproduces Photon's exact-size resize at each step.
    let mut target_width = original_width;
    let mut target_height = original_height;
    if target_width > MAX_IMAGE_EDGE {
        target_height =
            ((target_height as f64 * MAX_IMAGE_EDGE as f64) / target_width as f64).round() as u32;
        target_width = MAX_IMAGE_EDGE;
    }
    if target_height > MAX_IMAGE_EDGE {
        target_width =
            ((target_width as f64 * MAX_IMAGE_EDGE as f64) / target_height as f64).round() as u32;
        target_height = MAX_IMAGE_EDGE;
    }

    let mut current_width = target_width.max(1);
    let mut current_height = target_height.max(1);
    loop {
        let resized = decoded.resize_exact(
            current_width,
            current_height,
            image::imageops::FilterType::Lanczos3,
        );
        if let Some((data, mime)) = first_candidate_under_cap(&resized) {
            return Some(Resized {
                data,
                mime_type: mime.to_string(),
                original_width,
                original_height,
                width: current_width,
                height: current_height,
                was_resized: true,
            });
        }

        if current_width == 1 && current_height == 1 {
            break;
        }
        // Pi lines 146-153: shrink each axis by ×0.75 (floor, min 1); stop when neither axis moves.
        let next_width = if current_width == 1 {
            1
        } else {
            ((current_width as f64) * 0.75).floor().max(1.0) as u32
        };
        let next_height = if current_height == 1 {
            1
        } else {
            ((current_height as f64) * 0.75).floor().max(1.0) as u32
        };
        if next_width == current_width && next_height == current_height {
            break;
        }
        current_width = next_width;
        current_height = next_height;
    }
    None
}

/// Process an image faithfully to Pi `processImage` (image-process.ts:71-118): first `normalizeImage`
/// — KEEP the source MIME for the supported inline formats (PNG/JPEG/GIF/WebP), converting only
/// unsupported formats to PNG — then `resizeImage`, the byte-cap re-encode ladder ([`resize_image`])
/// that enforces the 4.5MB base64 limit AS WELL AS the 2000px edge. Emits the `[Image converted from …
/// to …]` hint (compared against the FINAL re-encoded MIME, Pi image-process.ts:96) and the `[Image:
/// original WxH, displayed at WxH. Multiply coordinates by S …]` dimension note when a resize occurred
/// (Pi `formatDimensionNote`, image-resize.ts:116-123). Returns [`ImageOmit`] on failure so the caller
/// emits Pi's matching placeholder (image-process.ts:80-92).
fn process_image(
    bytes: &[u8],
    detected_mime: &str,
    auto_resize: bool,
) -> Result<ProcessedImage, ImageOmit> {
    // normalizeImage: keep the source format when supported inline, else convert to PNG.
    let (norm_mime, norm_bytes, converted_from): (String, Vec<u8>, Option<String>) =
        match supported_inline_mime(detected_mime) {
            Some(mime) => (mime.to_string(), bytes.to_vec(), None),
            None => {
                // Pi `convertImageBytesToPng`; a decode/encode failure is the "could not be converted"
                // omission (image-process.ts:79-83).
                let decoded = image::load_from_memory(bytes).map_err(|_| ImageOmit::Convert)?;
                let mut buf = std::io::Cursor::new(Vec::new());
                decoded
                    .write_to(&mut buf, image::ImageFormat::Png)
                    .map_err(|_| ImageOmit::Convert)?;
                (
                    "image/png".to_string(),
                    buf.into_inner(),
                    Some(
                        detected_mime
                            .split(';')
                            .next()
                            .unwrap_or(detected_mime)
                            .trim()
                            .to_ascii_lowercase(),
                    ),
                )
            }
        };

    // `if (autoResizeImages) { … }` (image-process.ts). The FALSE branch skips `resizeImage`
    // entirely and inlines the normalized original bytes: no byte-cap ladder, no dimension note, and
    // the conversion hint compared against the NORMALIZED mime rather than a re-encoded one.
    if !auto_resize {
        let mut hints: Vec<String> = Vec::new();
        if let Some(from) = converted_from.as_ref()
            && from != &norm_mime
        {
            hints.push(format!("[Image converted from {from} to {norm_mime}.]"));
        }
        return Ok(ProcessedImage {
            data: base64::engine::general_purpose::STANDARD.encode(&norm_bytes),
            mime_type: norm_mime,
            hints,
        });
    }

    // resizeImage: the byte-cap re-encode ladder. `None` ⇒ the "could not be resized" omission.
    let resized = resize_image(&norm_bytes, &norm_mime).ok_or(ImageOmit::Resize)?;

    let mut hints: Vec<String> = Vec::new();
    // conversionHint(convertedFrom, resized.mimeType): compared against the FINAL MIME, so a BMP that
    // converts to PNG but re-encodes to JPEG reads "converted from image/bmp to image/jpeg"
    // (image-process.ts:67-69,96).
    if let Some(from) = converted_from.as_ref()
        && from != &resized.mime_type
    {
        hints.push(format!(
            "[Image converted from {from} to {}.]",
            resized.mime_type
        ));
    }
    // formatDimensionNote (image-resize.ts:116-123): only when a resize/re-encode occurred.
    if resized.was_resized {
        let scale = resized.original_width as f64 / resized.width.max(1) as f64;
        hints.push(format!(
            "[Image: original {}x{}, displayed at {}x{}. Multiply coordinates by {scale:.2} to map to original image.]",
            resized.original_width, resized.original_height, resized.width, resized.height
        ));
    }

    Ok(ProcessedImage {
        data: resized.data,
        mime_type: resized.mime_type,
        hints,
    })
}

/// Process the `@file` references into wrapped text + image attachments (Pi `processFileArguments`).
/// `auto_resize` is Pi's `options.autoResizeImages` (file-processor.ts:24-25), threaded from
/// `settingsManager.getImageAutoResize()` at main.ts:830 and handed to `processImage` at
/// file-processor.ts:53.
async fn process_file_args(
    files: &[String],
    cwd: &Path,
    auto_resize: bool,
) -> anyhow::Result<ProcessedFiles> {
    let mut out = ProcessedFiles::default();
    for spec in files {
        let abs = resolve_read_path(spec, cwd);
        // Missing file → hard error (file-processor.ts:37); the bin maps this to exit 1.
        let meta = match tokio::fs::metadata(&abs).await {
            Ok(m) => m,
            Err(_) => bail!("File not found: {}", abs.display()),
        };
        // Skip empty files (file-processor.ts:43).
        if meta.len() == 0 {
            continue;
        }
        let bytes = tokio::fs::read(&abs)
            .await
            .with_context(|| format!("Could not read file {}", abs.display()))?;
        let name = abs.display();
        match detect_image_mime(&bytes) {
            Some(mime) => match process_image(&bytes, mime, auto_resize) {
                Ok(processed) => {
                    out.images
                        .push(Content::Image { data: processed.data, mime_type: processed.mime_type });
                    // Reference the image with its processing hints (Pi file-processor.ts:67-72): the
                    // hint lines joined with "\n" inside the `<file>` tag, or an empty tag when none.
                    if processed.hints.is_empty() {
                        out.text.push_str(&format!("<file name=\"{name}\"></file>\n"));
                    } else {
                        out.text.push_str(&format!(
                            "<file name=\"{name}\">{}</file>\n",
                            processed.hints.join("\n")
                        ));
                    }
                }
                // Unprocessable image → text placeholder (Pi `processed.ok === false`,
                // file-processor.ts:55-58); the omission reason picks Pi's matching message
                // (image-process.ts:80-92).
                Err(reason) => out.text.push_str(&format!(
                    "<file name=\"{name}\">{}</file>\n",
                    reason.message()
                )),
            },
            None => {
                // Text file: wrap content in <file> tags with the absolute path.
                let content = String::from_utf8_lossy(&bytes);
                out.text.push_str(&format!("<file name=\"{name}\">\n{content}\n</file>\n"));
            }
        }
    }
    Ok(out)
}

/// Merge the three input sources into [`Inputs`] (pure; the file/stdin reads happen in
/// [`build_inputs`]). The initial prompt is `piped ⧺ file_text ⧺ messages[0]`, joined with `""`
/// (Pi initial-message.ts:27-40). The file wrapper supplies its own newlines, so NO separators are
/// added — the prompt bytes are identical to Pi's.
pub fn compose_inputs(
    file_text: Option<String>,
    images: Vec<Content>,
    messages: &[String],
    piped: Option<String>,
) -> Inputs {
    let mut parts: Vec<String> = Vec::new();
    if let Some(piped) = piped {
        parts.push(piped);
    }
    if let Some(text) = file_text
        && !text.is_empty()
    {
        parts.push(text);
    }
    if let Some(first) = messages.first() {
        parts.push(first.clone());
    }
    Inputs {
        initial: parts.concat(),
        images,
        follow_ups: messages.iter().skip(1).cloned().collect(),
    }
}

/// The value Pi's `readPipedStdin` resolves with: `data.trim() || undefined` (main.ts:80).
///
/// **The trim is load-bearing, not cosmetic.** [`compose_inputs`] joins its parts with `""` because
/// Pi's `buildInitialMessage` does (initial-message.ts:40) — and Pi can use an empty separator
/// precisely because the stdin half arrived already trimmed. cyrup used to test `buf.trim()` for
/// emptiness but return the UNTRIMMED `buf`, so `echo context | cyrup "summarise this"` sent
/// `"context\nsummarise this"` where Pi sends `"contextsummarise this"` — a divergence in the
/// literal prompt bytes handed to the model, with leading whitespace surviving at the front too.
///
/// Split out of [`read_piped_stdin`] so the byte-level contract is testable without a real pipe;
/// the read itself contributes nothing but the string.
fn normalize_piped_stdin(buf: &str) -> Option<String> {
    let trimmed = buf.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Read piped stdin to a string when stdin is not a TTY (R-11-006); `None` when interactive or empty.
/// The text is trimmed before it reaches the prompt (Pi `data.trim() || undefined`, main.ts:80) —
/// see [`normalize_piped_stdin`].
///
/// **Public, and called from `main.rs` rather than from [`build_inputs`], because that is Pi's own
/// shape**: `main.ts:819-826` reads `stdinContent = await readPipedStdin()` in `main` (skipping it
/// entirely for RPC mode, which owns stdin for JSON-RPC) and `:828-832` then *passes* the string
/// into `prepareInitialMessage(parsed, autoResize, stdinContent)`. Folding the read inside
/// `build_inputs` made the whole prompt-assembly path depend on a process-global descriptor: under
/// `cargo test` a test binary inherits whatever stdin the runner has, and if that is a pipe nobody
/// closes, `read_to_string` never returns and the test target hangs forever instead of failing.
/// See area-08 `SEAM-072`.
pub async fn read_piped_stdin() -> anyhow::Result<Option<String>> {
    if std::io::stdin().is_terminal() {
        return Ok(None);
    }
    let mut buf = String::new();
    tokio::io::stdin()
        .read_to_string(&mut buf)
        .await
        .context("reading piped stdin")?;
    Ok(normalize_piped_stdin(&buf))
}

/// Build the prompt inputs from the CLI: split positionals, process `@file` text + images, merge
/// piped stdin. `cwd` resolves relative `@file` paths (Pi uses `process.cwd()`).
///
/// `auto_resize` is the effective `images.autoResize` setting, mirroring Pi's
/// `prepareInitialMessage(parsed, settingsManager.getImageAutoResize(), stdinContent)`
/// (main.ts:828-832 → :181 → file-processor.ts:53). It is a required argument rather than a
/// defaulted option precisely because it used to be missing: `@screenshot.png` always downsampled to
/// 2000px and injected a `[Image: original …, displayed at …]` note no matter what the settings
/// panel's "Auto-resize images" toggle said.
///
/// `piped` is the already-read piped-stdin content — Pi's third `prepareInitialMessage` argument
/// (`stdinContent`, main.ts:831), produced by [`read_piped_stdin`] at the call site. It is a
/// parameter and not an internal read for the same reason it is one upstream: this function is the
/// prompt-assembly step, not the descriptor-owning step.
pub async fn build_inputs(
    cli: &Cli,
    cwd: &Path,
    auto_resize: bool,
    piped: Option<String>,
) -> anyhow::Result<Inputs> {
    let (files, messages) = split_positionals(&cli.positionals);
    let processed = process_file_args(&files, cwd, auto_resize).await?;
    let file_text = if processed.text.is_empty() {
        None
    } else {
        Some(processed.text)
    };
    Ok(compose_inputs(
        file_text,
        processed.images,
        &messages,
        piped,
    ))
}

/// The `@file` references in the CLI positionals (Pi `parsed.fileArgs`). Used to reject `@file` in
/// RPC mode (main.ts:540).
pub fn split_file_args(cli: &Cli) -> Vec<String> {
    split_positionals(&cli.positionals).0
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn split_separates_files_from_messages() {
        let (files, messages) = split_positionals(&s(&["@a.txt", "hello", "world", "@dir/b.md"]));
        assert_eq!(files, s(&["a.txt", "dir/b.md"]));
        assert_eq!(messages, s(&["hello", "world"]));
    }

    #[test]
    fn double_at_is_a_literal_message() {
        let (files, messages) = split_positionals(&s(&["@@handle", "hi"]));
        assert!(files.is_empty());
        assert_eq!(messages, s(&["@handle", "hi"]));
    }

    #[test]
    fn compose_uses_empty_join_with_stdin_first() {
        // Pi order: stdin ⧺ fileText ⧺ message0, joined "". The file wrapper supplies its own \n.
        let inputs = compose_inputs(
            Some("<file name=\"/a\">\nBODY\n</file>\n".to_string()),
            Vec::new(),
            &s(&["first", "second", "third"]),
            Some("PIPED\n".to_string()),
        );
        assert_eq!(
            inputs.initial,
            "PIPED\n<file name=\"/a\">\nBODY\n</file>\nfirst"
        );
        assert_eq!(inputs.follow_ups, s(&["second", "third"]));
    }

    #[test]
    fn compose_handles_message_only_and_stdin_only() {
        let only_msg = compose_inputs(None, Vec::new(), &s(&["just a message"]), None);
        assert_eq!(only_msg.initial, "just a message");
        assert!(only_msg.follow_ups.is_empty());

        let only_stdin = compose_inputs(None, Vec::new(), &[], Some("from stdin".to_string()));
        assert_eq!(only_stdin.initial, "from stdin");
        assert!(!only_stdin.is_empty());

        let nothing = compose_inputs(None, Vec::new(), &[], None);
        assert!(nothing.is_empty());
    }

    #[tokio::test]
    async fn text_file_is_wrapped_in_file_tags_with_absolute_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "hello world").unwrap();
        let processed = process_file_args(&[path.to_string_lossy().into_owned()], dir.path(), true)
            .await
            .unwrap();
        let expected = format!("<file name=\"{}\">\nhello world\n</file>\n", path.display());
        assert_eq!(processed.text, expected);
        assert!(processed.images.is_empty());
    }

    #[tokio::test]
    async fn empty_file_is_skipped_and_missing_file_errors() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty.txt");
        std::fs::write(&empty, "").unwrap();
        let processed = process_file_args(&[empty.to_string_lossy().into_owned()], dir.path(), true)
            .await
            .unwrap();
        assert!(processed.text.is_empty());

        let err = process_file_args(&["does-not-exist.txt".to_string()], dir.path(), true)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("File not found"));
    }

    #[tokio::test]
    async fn png_file_is_attached_as_image_with_empty_file_ref() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dot.png");
        // A 1×1 PNG via the image crate so the magic bytes + decode path are real.
        let img = image::RgbaImage::from_pixel(1, 1, image::Rgba([10, 20, 30, 255]));
        img.save(&path).unwrap();
        let processed = process_file_args(&[path.to_string_lossy().into_owned()], dir.path(), true)
            .await
            .unwrap();
        assert_eq!(processed.images.len(), 1);
        match &processed.images[0] {
            Content::Image { mime_type, data } => {
                assert_eq!(mime_type, "image/png");
                assert!(!data.is_empty());
            }
            other => panic!("expected image content, got {other:?}"),
        }
        assert_eq!(
            processed.text,
            format!("<file name=\"{}\"></file>\n", path.display())
        );
    }

    #[test]
    fn detect_mime_recognizes_signatures() {
        assert_eq!(
            detect_image_mime(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]),
            Some("image/png")
        );
        assert_eq!(
            detect_image_mime(&[0xff, 0xd8, 0xff, 0x00]),
            Some("image/jpeg")
        );
        assert_eq!(detect_image_mime(b"GIF89a..."), Some("image/gif"));
        assert_eq!(detect_image_mime(b"plain text"), None);
    }
}
