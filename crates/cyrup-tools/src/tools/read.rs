//! `read` — text window + image attachment (R-03-011…014, arch-03 §6.3). One-shot, no streaming.

use crate::config::ReadOpts;
use crate::details::ReadDetails;
use crate::ops::FsOps;
use crate::truncate::{DEFAULT_MAX_BYTES, TruncOpts, format_size, truncate_head};
use crate::{error, path};
use cyrup_core::{CancelToken, Content, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadInput {
    path: String,
    // Pi's TypeBox `Type.Number` (read.ts:22-23) carries no `integer` and no `minimum`, and Pi
    // never validates tool arguments at runtime, so `offset: 10.0` and `limit: -1` are inputs Pi
    // accepts and coerces. Modeling these as `usize` (the old cyrup type) rejected the entire
    // call at deserialization. See [`crate::jsnum`]; `bash`'s `timeout` (bash.rs:24) is the same
    // fix applied earlier.
    offset: Option<f64>,
    limit: Option<f64>,
}

pub struct ReadTool {
    fs: Arc<dyn FsOps>,
    cwd: PathBuf,
    opts: ReadOpts,
    params: serde_json::Value,
}

impl ReadTool {
    pub fn new(fs: Arc<dyn FsOps>, cwd: PathBuf, opts: ReadOpts) -> Self {
        // Schema is byte-for-byte Pi's TypeBox emission (read.ts:20-24): verbatim property
        // descriptions, `type:"number"` (not integer), NO `minimum`, and NO `additionalProperties`
        // (TypeBox only sets it where the source passes `{ additionalProperties: false }`, which NO
        // built-in does — `edit` passes an empty `{}`, see `edit.rs`). This object IS the
        // model-facing `input_schema`.
        let params = serde_json::json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": { "type": "string", "description": "Path to the file to read (relative or absolute)" },
                "offset": { "type": "number", "description": "Line number to start reading from (1-indexed)" },
                "limit": { "type": "number", "description": "Maximum number of lines to read" }
            }
        });
        Self {
            fs,
            cwd,
            opts,
            params,
        }
    }
}

#[async_trait::async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str {
        "read"
    }
    /// TOOL-045. Pi's built-in `ToolDefinition`s all declare `label` EXPLICITLY next to `name`, and
    /// for all seven the two strings are equal — `read.ts:210-211` @v0.83.0, and the same adjacent
    /// pair at `bash.ts:325-326`, `edit.ts:293-294`, `write.ts:187-188`, `grep.ts:129-130`,
    /// `find.ts:115-116`, `ls.ts:101-102`.
    ///
    /// Leaving these to `Tool::label`'s `None` default was behaviourally equivalent *today* (the
    /// fallback yields the name), but it meant the field was declared on the trait and set by NO
    /// built-in, so the fallback had never been exercised against a label that differs from the
    /// name and nothing downstream was proven to read the declared value. Declaring it makes the
    /// seven a byte-diffable port of pi's literal definitions rather than an inference from a
    /// default.
    fn label(&self) -> Option<&str> {
        Some("read")
    }
    fn parameters(&self) -> &serde_json::Value {
        &self.params
    }

    // Verbatim from Pi (read.ts:212-214). DEFAULT_MAX_LINES=2000, DEFAULT_MAX_BYTES/1024=50.
    fn description(&self) -> &str {
        "Read the contents of a file. Supports text files and images (jpg, png, gif, webp, bmp). \
         Images are sent as attachments. For text files, output is truncated to 2000 lines or 50KB \
         (whichever is hit first). Use offset/limit for large files. When you need the full file, \
         continue with offset until complete."
    }
    fn prompt_snippet(&self) -> Option<&str> {
        Some("Read file contents")
    }
    fn prompt_guidelines(&self) -> Vec<&str> {
        vec!["Use read to examine files instead of cat or sed."]
    }

    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: serde_json::Value,
        cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let input: ReadInput =
            serde_json::from_value(params).map_err(|e| error::invalid(format!("read: {e}")))?;

        // Resolve to an existing candidate (macOS variant fallback, R-03-006). Pi
        // `resolveReadPathAsync` SELECTS the first variant that EXISTS (`F_OK`) and falls back to
        // the primary if none exist (read.ts:238, path-utils.ts:86-118). Readability is then a
        // SEPARATE `R_OK` check on the CHOSEN path — it does NOT continue probing other variants
        // (read.ts:241). So a primary that exists-but-is-unreadable errors even when a readable
        // variant follows (UM-6).
        let candidates = path::resolve_read_path(&input.path, &self.cwd);
        let mut abs = None;
        for cand in &candidates {
            if self
                .fs
                .access(cand, crate::ops::Access::Exists)
                .await
                .is_ok()
            {
                abs = Some(cand.clone());
                break;
            }
        }
        // None exist ⇒ Pi keeps the primary (candidates[0]); the R_OK check below then fails.
        let abs = abs.unwrap_or_else(|| candidates.first().cloned().unwrap_or_default());
        // Pi does NOT wrap this failure: `await ops.access(absolutePath)` (read.ts:241) is
        // uncaught — `execute`'s only catch re-`reject`s the original error (read.ts:321-324) — so
        // the model sees Node's raw errno text, carrying both the errno CODE and the RESOLVED
        // absolute path (`ENOENT: no such file or directory, access '/work/missing.txt'`). Note the
        // sibling `edit` deliberately does wrap (edit.ts:326-331), which `edit.rs:194-196` mirrors;
        // `read` is the one that must propagate. Substituting a fixed
        // "File not found or unreadable: {input.path}" collapsed ENOENT/EACCES/ENOTDIR into one
        // string and reported the raw user-supplied path — misleading precisely because the loop
        // above may have selected a macOS filename VARIANT of it. `LocalFs::access` already builds
        // `"{resolved path}: {io error}"` (ops/local.rs:113), so propagating is enough.
        self.fs.access(&abs, crate::ops::Access::Read).await?;

        if cancel.is_cancelled() {
            return Err(error::aborted());
        }

        // Read once through the (remote-aware) seam, then decide text-vs-image by MAGIC BYTES — Pi
        // sniffs the file header (read.ts:243 → mime.ts), not the extension.
        let bytes = self.fs.read(&abs).await?;

        // Image branch (R-03-012). The sniff sees only the first `IMAGE_TYPE_SNIFF_BYTES` (4100)
        // bytes, which is the window Pi's `detectSupportedImageMimeTypeFromFile` reads
        // (mime.ts:28-30) before calling the same predicate — NOT the whole file. Handing the
        // whole file to the sniffer let `isAnimatedPng`'s chunk walk (mime.ts:42-55) find an
        // `acTL` past byte 4100, where Pi's walk has already bailed at `:51`; the file then fell
        // through to the TEXT branch below and the model got `from_utf8_lossy` of a PNG instead of
        // the picture Pi shows.
        if let Some(mime) = crate::ops::ImageMime::from_file_head(&bytes) {
            return self.read_image(bytes, mime).await;
        }

        // Text branch (R-03-011).
        let text = String::from_utf8_lossy(&bytes).into_owned();
        // Pi's basis is `allLines.length` — the raw `split("\n")` count, which INCLUDES the empty
        // phantom element after a trailing newline (read.ts:268-269). Do not pop it: the offset
        // bound, the `of N` continuation count, and the out-of-bounds error all key off this count.
        let lines: Vec<&str> = text.split('\n').collect();
        let total = lines.len();

        // Pi: `const startLine = offset ? Math.max(0, offset - 1) : 0` (read.ts:271). `offset` is a
        // JS float: a falsy `0` and a negative both land on `0` via the `Math.max`, and `NaN`
        // likewise (`f64::max` returns the non-NaN operand, matching `Math.max(0, NaN - 1)`… which
        // is NaN in JS, but `offset` being NaN is unreachable from JSON). A fractional offset is
        // truncated toward zero, which is what `allLines.slice(startLine, …)` (read.ts:283) does
        // with it downstream.
        let start = crate::jsnum::to_count(input.offset.map_or(0.0, |o| (o - 1.0).max(0.0)));
        if start >= total {
            // Pi interpolates the RAW argument here (read.ts:275), not the clamped index. Rust's
            // `f64` Display matches JS number-to-string for the integral values this sees (`3.0`
            // renders as `3` in both).
            return Err(error::invalid(format!(
                "Offset {} is beyond end of file ({} lines total)",
                input.offset.unwrap_or(0.0),
                total
            )));
        }

        // Pi: `const endLine = Math.min(startLine + limit, allLines.length)` (read.ts:282). The add
        // is unclamped in JS, so a negative `limit` makes `endLine < startLine`; `slice` then
        // applies its count-from-the-end rule and the continuation notice below quotes a negative
        // `offset=`. [CYRUP-DELTA]: cyrup clamps the window end into `[start, total]`, so a
        // negative `limit` yields an empty window and a notice that points back at `start + 1`.
        // Byte-identical to Pi for every non-negative `limit`; a fractional one truncates toward
        // zero exactly as `slice` would.
        let end = match input.limit {
            #[allow(clippy::cast_precision_loss)]
            Some(l) => crate::jsnum::to_count(start as f64 + l).clamp(start, total),
            None => total,
        };
        let window: Vec<&str> = lines
            .get(start..end)
            .map(<[&str]>::to_vec)
            .unwrap_or_default();
        let window_text = window.join("\n");

        let t = truncate_head(
            &window_text,
            TruncOpts::new(self.opts.max_lines, self.opts.max_bytes),
        );

        if t.info.first_line_exceeds_limit {
            // Pi resolves SUCCESSFULLY here (read.ts:290-294,315): the note is the content and the
            // truncation is attached as `details`, so the model gets an actionable result, not an
            // `isError` failure. `firstLineSize` is the byte length of the first selected line.
            let line_no = start + 1;
            let first_line_bytes = window.first().map_or(0, |l| l.len());
            // Pi hardcodes `formatSize(DEFAULT_MAX_BYTES)` and `head -c ${DEFAULT_MAX_BYTES}` here
            // (read.ts:293), independent of any configured limit. Use the fixed constant.
            let out = format!(
                "[Line {line_no} is {}, exceeds {} limit. Use bash: sed -n '{line_no}p' {} | head -c {}]",
                format_size(first_line_bytes),
                format_size(DEFAULT_MAX_BYTES),
                input.path,
                DEFAULT_MAX_BYTES,
            );
            return Ok(ToolResult {
                content: vec![Content::text(out)],
                details: serde_json::to_value(ReadDetails {
                    truncation: Some(t.info),
                })
                .ok(),
                terminate: false,
                ..Default::default()
            });
        }

        let mut out = t.content.clone();
        if t.info.truncated {
            let shown_to = start + t.info.output_lines;
            // Pi distinguishes line- vs byte-triggered truncation in the continuation note
            // (read.ts:300-304): the byte case appends the `(50.0KB limit)` qualifier.
            if t.info.truncated_by == Some(crate::truncate::TruncatedBy::Lines) {
                out.push_str(&format!(
                    "\n\n[Showing lines {}-{} of {}. Use offset={} to continue.]",
                    start + 1,
                    shown_to,
                    total,
                    shown_to + 1
                ));
            } else {
                // Pi's byte-case qualifier hardcodes `formatSize(DEFAULT_MAX_BYTES)` (read.ts:303).
                out.push_str(&format!(
                    "\n\n[Showing lines {}-{} of {} ({} limit). Use offset={} to continue.]",
                    start + 1,
                    shown_to,
                    total,
                    format_size(DEFAULT_MAX_BYTES),
                    shown_to + 1
                ));
            }
        } else if end < total {
            let remaining = total - end;
            out.push_str(&format!(
                "\n\n[{remaining} more lines in file. Use offset={} to continue.]",
                end + 1
            ));
        }

        // Pi only sets `details` on the firstLineExceeds and truncated branches; the user-limited
        // and plain branches leave it `undefined` (read.ts:294-315). Mirror that.
        let details = if t.info.truncated {
            serde_json::to_value(ReadDetails {
                truncation: Some(t.info),
            })
            .ok()
        } else {
            None
        };
        Ok(ToolResult {
            content: vec![Content::text(out)],
            details,
            terminate: false,
            ..Default::default()
        })
    }
}

impl ReadTool {
    /// Faithful port of Pi's image read path (read.ts:247-263). The model-facing note is
    /// `Read image file [<mime>]` plus any processing hints, and — for non-vision models — the
    /// image block is STILL returned together with a warning note (Pi keeps the block; the request
    /// layer strips it later). `mime` is the magic-byte-detected type.
    async fn read_image(
        &self,
        bytes: Vec<u8>,
        mime: crate::ops::ImageMime,
    ) -> Result<ToolResult, ToolError> {
        // `getNonVisionImageNote` (read.ts:87-92), evaluated PER CALL exactly like Pi's
        // `getNonVisionImageNote(ctx?.model)` (read.ts:246) — `supports_images_now()` prefers the
        // live `ModelVisionHandle` the session layer owns, so a mid-session `/model` switch to a
        // text-only model reaches the very next `read` instead of the construction-time value.
        let non_vision_note: Option<&str> = if self.opts.supports_images_now() {
            None
        } else {
            Some(
                "[Current model does not support images. The image will be omitted from this request.]",
            )
        };

        #[cfg(feature = "inline-images")]
        {
            match image_proc::process_image(
                &bytes,
                mime,
                self.opts.max_image_dim,
                self.opts.auto_resize_images,
            ) {
                image_proc::Processed::Ok {
                    data,
                    mime: out_mime,
                    hints,
                } => {
                    // `Read image file [${processed.mimeType}]` + hints + nonVisionNote.
                    let mut note = format!("Read image file [{out_mime}]");
                    for h in &hints {
                        note.push('\n');
                        note.push_str(h);
                    }
                    if let Some(nv) = non_vision_note {
                        note.push('\n');
                        note.push_str(nv);
                    }
                    Ok(ToolResult {
                        content: vec![
                            Content::text(note),
                            Content::Image {
                                data,
                                mime_type: out_mime,
                            },
                        ],
                        details: None,
                        terminate: false,
                        ..Default::default()
                    })
                }
                image_proc::Processed::Failed { message } => {
                    // `Read image file [${mimeType}]\n${message}` + nonVisionNote (no image block).
                    let mut note = format!("Read image file [{}]\n{message}", mime.mime());
                    if let Some(nv) = non_vision_note {
                        note.push('\n');
                        note.push_str(nv);
                    }
                    Ok(ToolResult {
                        content: vec![Content::text(note)],
                        details: None,
                        terminate: false,
                        ..Default::default()
                    })
                }
            }
        }

        #[cfg(not(feature = "inline-images"))]
        {
            // Image decoding is only compiled out under `--no-default-features`; the default build
            // always inlines. Surface the detected type + a build note (and the non-vision note).
            let mut note = format!(
                "Read image file [{}] ({}).\n[Image inlining is not enabled in this build (feature `inline-images`).]",
                mime.mime(),
                format_size(bytes.len())
            );
            if let Some(nv) = non_vision_note {
                note.push('\n');
                note.push_str(nv);
            }
            Ok(ToolResult {
                content: vec![Content::text(note)],
                details: None,
                terminate: false,
                ..Default::default()
            })
        }
    }
}

/// Image normalize+resize, a faithful port of Pi's `processImage`/`resizeImageInProcess`
/// (image-process.ts, image-resize-core.ts). Decodes via the `image` crate (already in the
/// lockfile), applies EXIF orientation, preserves the source format when it is an inline-supported
/// type, converts unsupported types (bmp) to PNG, and resizes to fit 2000x2000 / 4.5MB of base64.
#[cfg(feature = "inline-images")]
mod image_proc {
    use super::base64_encode;
    use crate::ops::ImageMime;
    use image::{DynamicImage, ImageDecoder};
    use std::io::Cursor;

    /// 4.5MB of base64 payload — Pi's headroom below Anthropic's 5MB limit (image-resize-core.ts:22).
    const MAX_B64_BYTES: usize = 4_718_592;
    /// Pi's default JPEG quality (image-resize-core.ts:28) + its descending retry ladder (line 122).
    const JPEG_QUALITIES: [u8; 5] = [80, 85, 70, 55, 40];

    pub enum Processed {
        Ok {
            data: String,
            mime: String,
            hints: Vec<String>,
        },
        Failed {
            message: String,
        },
    }

    struct Resized {
        data: String,
        mime: String,
        original_width: u32,
        original_height: u32,
        width: u32,
        height: u32,
        was_resized: bool,
    }

    /// `processImage` (image-process.ts:72-119). `auto_resize` is Pi's `options.autoResizeImages`,
    /// threaded down from the `images.autoResize` setting: `true` runs `normalizeImage` then the
    /// `resizeImage` ladder; `false` normalizes ONLY and inlines the original bytes, with the
    /// conversion hint compared against the NORMALIZED mime (never a re-encoded one) and no
    /// dimension note, exactly like image-process.ts's trailing else-branch.
    pub fn process_image(
        orig: &[u8],
        detected: ImageMime,
        max_dim: u32,
        auto_resize: bool,
    ) -> Processed {
        // normalizeImage (image-process.ts:49-65): keep supported inline formats as-is; convert
        // everything else (bmp) to PNG, baking EXIF orientation in.
        let (norm_bytes, norm_mime, converted_from): (std::borrow::Cow<[u8]>, &str, Option<&str>) =
            match detected {
                ImageMime::Png => (std::borrow::Cow::Borrowed(orig), "image/png", None),
                ImageMime::Jpeg => (std::borrow::Cow::Borrowed(orig), "image/jpeg", None),
                ImageMime::Gif => (std::borrow::Cow::Borrowed(orig), "image/gif", None),
                ImageMime::Webp => (std::borrow::Cow::Borrowed(orig), "image/webp", None),
                ImageMime::Bmp => match convert_to_png(orig) {
                    Some(png) => (std::borrow::Cow::Owned(png), "image/png", Some("image/bmp")),
                    None => {
                        return Processed::Failed {
                            message:
                                "[Image omitted: could not be converted to a supported inline image format.]"
                                    .to_string(),
                        }
                    }
                },
            };

        // `if (autoResizeImages) { … }` — the false path returns the normalized bytes base64-encoded
        // with no resize, no byte-cap ladder and no dimension note (image-process.ts, final block).
        if !auto_resize {
            let mut hints: Vec<String> = Vec::new();
            if let Some(from) = converted_from
                && from != norm_mime
            {
                hints.push(format!("[Image converted from {from} to {norm_mime}.]"));
            }
            return Processed::Ok {
                data: base64_encode(&norm_bytes),
                mime: norm_mime.to_string(),
                hints,
            };
        }

        match resize_image(&norm_bytes, norm_mime, max_dim) {
            Some(r) => {
                let mut hints: Vec<String> = Vec::new();
                // conversionHint (image-process.ts:67-70).
                if let Some(from) = converted_from
                    && from != r.mime
                {
                    hints.push(format!("[Image converted from {from} to {}.]", r.mime));
                }
                // formatDimensionNote (image-resize.ts:116-123).
                if r.was_resized {
                    let scale = f64::from(r.original_width) / f64::from(r.width.max(1));
                    hints.push(format!(
                        "[Image: original {}x{}, displayed at {}x{}. Multiply coordinates by {:.2} \
                         to map to original image.]",
                        r.original_width, r.original_height, r.width, r.height, scale
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

    /// `convertImageBytesToPng` (image-convert.ts:4-24): decode (EXIF-oriented) + re-encode PNG.
    fn convert_to_png(bytes: &[u8]) -> Option<Vec<u8>> {
        let img = decode_with_orientation(bytes)?;
        encode_png(&img)
    }

    /// Decode + apply EXIF orientation (Pi `applyExifOrientation`). The `image` crate exposes the
    /// decoder's EXIF orientation (0.25+) which we bake into the pixels.
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
        let rgb = img.to_rgb8();
        let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);
        enc.encode_image(&rgb).ok()?;
        Some(buf)
    }

    /// `resizeImageInProcess` (image-resize-core.ts:59-164). Returns `None` only when the image
    /// cannot be brought under the base64 budget even at 1x1 (or decode fails).
    fn resize_image(bytes: &[u8], mime: &str, max_dim: u32) -> Option<Resized> {
        let input_base64_size = bytes.len().div_ceil(3) * 4;
        let img = decode_with_orientation(bytes)?;
        let original_width = img.width();
        let original_height = img.height();

        // Already within all limits ⇒ send the ORIGINAL (normalized) bytes untouched.
        if original_width <= max_dim
            && original_height <= max_dim
            && input_base64_size < MAX_B64_BYTES
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

        // Initial target dims, preserving aspect ratio (image-resize-core.ts:96-106).
        let (mut target_w, mut target_h) = (original_width, original_height);
        if target_w > max_dim {
            target_h =
                ((f64::from(target_h) * f64::from(max_dim)) / f64::from(target_w)).round() as u32;
            target_w = max_dim;
        }
        if target_h > max_dim {
            target_w =
                ((f64::from(target_w) * f64::from(max_dim)) / f64::from(target_h)).round() as u32;
            target_h = max_dim;
        }

        let (mut cw, mut ch) = (target_w.max(1), target_h.max(1));
        loop {
            let resized = img.resize_exact(cw, ch, image::imageops::FilterType::Lanczos3);
            // Candidate order (image-resize-core.ts:112-115): PNG first, then JPEG by quality.
            if let Some(png) = encode_png(&resized) {
                let data = base64_encode(&png);
                if data.len() < MAX_B64_BYTES {
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
            for q in dedup_qualities() {
                if let Some(jpg) = encode_jpeg(&resized, q) {
                    let data = base64_encode(&jpg);
                    if data.len() < MAX_B64_BYTES {
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

    fn dedup_qualities() -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        for q in JPEG_QUALITIES {
            if !out.contains(&q) {
                out.push(q);
            }
        }
        out
    }
}

/// Minimal RFC 4648 standard base64 (matches Node's `Buffer.toString("base64")`).
#[cfg(feature = "inline-images")]
fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = *chunk.first().unwrap_or(&0);
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        let n = ((b0 as u32) << 16) | ((b1 as u32) << 8) | (b2 as u32);
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
