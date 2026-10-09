//! Prompt-input assembly: positionals + `@file` + piped stdin (arch-11 §6.2; R-11-006/024/025).
//!
//! A 1:1 port of Pi `cli/file-processor.ts` + `cli/initial-message.ts`: `@`-prefixed positionals are
//! file references. Each text file is wrapped `<file name="ABS">\n{content}\n</file>\n`
//! (file-processor.ts:77); each image file is MIME-sniffed and handed to the ONE shared
//! `cyrup_tools::image_proc::process_image` (Pi `processImage`) — which, when the effective
//! `images.autoResize` setting is on, downscales it to fit the resize profile's clamps AND
//! re-encodes it below the profile's base64 cap, and when off inlines the normalized original bytes
//! verbatim (Pi threads `settingsManager.getImageAutoResize()` main.ts:830 → file-processor.ts:53).
//! No profile is supplied at this boundary — it is argv, reached before any model is selected, so
//! every key resolves to `DEFAULT_IMAGE_RESIZE` (2000×2000, 4.5 MiB of base64, quality 80), exactly
//! as Pi's `file-processor.ts:54` passes no `resizeOptions`. The result is attached as a base64
//! `Content::Image`, and
//! referenced with an empty `<file name="ABS"></file>\n` tag
//! (file-processor.ts:48-72). Empty files are skipped (file-processor.ts:43); a missing file is a
//! hard error the bin maps to exit 1 (file-processor.ts:37). The initial message is
//! `stdin ⧺ fileText ⧺ messages[0]` joined with `""` (initial-message.ts:27-40) — the file wrapper
//! already supplies its own newlines, so no separators are added.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use cyrup_sdk::core::Content;
use tokio::io::AsyncReadExt;

use crate::cli::Cli;

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
        // SEAM-133 — the sniff is pi's `detectSupportedImageMimeTypeFromFile`
        // (file-processor.ts:49 @v0.87.1), whose faithful port already lives in
        // `cyrup_tools::ImageMime::from_file_head`: it applies pi's 4 100-byte sniff window to a
        // buffer the caller already holds, rejects lossless JPEG (`FF D8 FF F7`) and animated PNG,
        // requires a real IHDR, and structurally validates a BMP header. `input.rs` previously
        // carried a second, laxer copy that called any `BM`-prefixed file a bitmap (so a text file
        // beginning `BMW …` reached the model as an image-processing failure instead of its text)
        // and claimed `FF D8 FF F7` as JPEG. There is now one copy of this predicate.
        match cyrup_tools::ImageMime::from_file_head(&bytes) {
            // SEAM-128 — one `process_image` for the whole tree. This path used to carry its own
            // ~215-line copy of Pi's `processImage`/`resizeImageInProcess`, which (a) had no EXIF
            // orientation handling at all, so a rotated phone screenshot reached the model
            // sideways where upstream bakes the tag in (`image-resize-core.ts:72`), and (b)
            // hardcoded its profile as `MAX_IMAGE_EDGE`/`MAX_IMAGE_BASE64_BYTES`/
            // `JPEG_QUALITY_STEPS` consts with no `jpegQuality` parameter at all. The shared
            // module takes the profile as `Option<&ModelImageResizeOptions>`.
            //
            // `resize: None` here is not a stub, it is the only honest value: this is the CLI
            // ARGV boundary, reached before any session exists and therefore before any model is
            // selected — Pi's own `file-processor.ts:54` likewise passes no `resizeOptions`. An
            // absent profile resolves per key against `DEFAULT_IMAGE_RESIZE`, i.e. byte-for-byte
            // the 2000px / 4.5 MiB / quality-80 numbers the deleted consts held.
            Some(mime) => match cyrup_tools::image_proc::process_image(
                &bytes,
                mime.mime(),
                cyrup_tools::image_proc::ProcessImageOptions {
                    auto_resize_images: auto_resize,
                    resize: None,
                },
            ) {
                cyrup_tools::image_proc::Processed::Ok {
                    data,
                    mime: out_mime,
                    hints,
                } => {
                    out.images.push(Content::Image {
                        data,
                        mime_type: out_mime,
                    });
                    // Reference the image with its processing hints (Pi file-processor.ts:67-72): the
                    // hint lines joined with "\n" inside the `<file>` tag, or an empty tag when none.
                    if hints.is_empty() {
                        out.text
                            .push_str(&format!("<file name=\"{name}\"></file>\n"));
                    } else {
                        out.text.push_str(&format!(
                            "<file name=\"{name}\">{}</file>\n",
                            hints.join("\n")
                        ));
                    }
                }
                // Unprocessable image → text placeholder (Pi `processed.ok === false`,
                // file-processor.ts:55-58). The message now comes from the shared module's
                // `Failed { message }` arm rather than being reconstructed here from a
                // two-variant enum — which is what lets it carry a per-call message at all.
                cyrup_tools::image_proc::Processed::Failed { message } => out
                    .text
                    .push_str(&format!("<file name=\"{name}\">{message}</file>\n")),
            },
            None => {
                // Text file: wrap content in <file> tags with the absolute path. A `null` sniff
                // means TEXT upstream too (file-processor.ts:75-77 @v0.87.1), and pi's read is
                // `stripBom(await readFile(absolutePath, "utf-8"))` — so a UTF-8 BOM is removed
                // before the content reaches the prompt rather than surviving as a U+FEFF inside
                // the `<file>` body.
                let body = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
                let content = String::from_utf8_lossy(body);
                out.text
                    .push_str(&format!("<file name=\"{name}\">\n{content}\n</file>\n"));
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
/// # The `@file` resize is the SESSION's job, not this function's (SEAM-128)
///
/// There is deliberately no `auto_resize` parameter here, and the one this function hands
/// [`process_file_args`] is a hard `false`. That is upstream's own shape: `prepareInitialMessage`
/// took an `autoResizeImages` argument until pi dropped it, and now calls
/// `processFileArguments(parsed.fileArgs, { autoResizeImages: false })` with the comment
/// *"AgentSession resizes these after extension hooks select the request model"*
/// (`packages/coding-agent/src/main.ts:221-223` @v1.0.4).
///
/// The reason is the whole point of SEAM-128. This is the ARGV boundary: it runs before a session
/// exists, so before any model is selected and before any `before_agent_start` handler has had the
/// chance to select another one. A resize applied here is applied against a profile nobody chose —
/// which is exactly what cyrup used to do, with a hardcoded 2000px / 4.5 MiB profile baked into
/// `input.rs` as consts. `AgentSession::normalize_prompt_images` now resizes every prompt image,
/// from every entry point, against the REQUEST model's `inputLimits.images.resize`; doing it twice
/// would re-encode an already-downscaled image for no gain and against the wrong profile.
///
/// `process_file_args` keeps its `auto_resize` parameter and its true branch, exactly as pi keeps
/// `processFileArguments`' option — only the CALL here stops asking for it.
///
/// `piped` is the already-read piped-stdin content — Pi's third `prepareInitialMessage` argument
/// (`stdinContent`, main.ts:831), produced by [`read_piped_stdin`] at the call site. It is a
/// parameter and not an internal read for the same reason it is one upstream: this function is the
/// prompt-assembly step, not the descriptor-owning step.
pub async fn build_inputs(cli: &Cli, cwd: &Path, piped: Option<String>) -> anyhow::Result<Inputs> {
    let (files, messages) = split_positionals(&cli.positionals);
    // `{ autoResizeImages: false }` — pi main.ts:223. See this function's docs.
    let processed = process_file_args(&files, cwd, false).await?;
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
        let processed =
            process_file_args(&[empty.to_string_lossy().into_owned()], dir.path(), true)
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

    /// SEAM-128 — `process_file_args` KEEPS its `auto_resize` parameter and its true branch, as pi
    /// keeps `processFileArguments`' `autoResizeImages` option (`cli/file-processor.ts:24-25,53`);
    /// only [`build_inputs`]'s CALL stopped asking for it (pi `main.ts:223`). This pins the branch
    /// so the parameter cannot rot into a dead argument: with it on, the same 2600px fixture
    /// `tests/image_auto_resize_file_args.rs` inlines verbatim is downscaled and annotated.
    #[tokio::test]
    async fn process_file_args_still_resizes_when_asked_to() {
        let dir = tempfile::tempdir().unwrap();
        let img: image::ImageBuffer<image::Rgb<u8>, Vec<u8>> =
            image::ImageBuffer::from_fn(2600, 800, |x, y| {
                image::Rgb([(x % 251) as u8, (y % 241) as u8, 0])
            });
        let path = dir.path().join("wide.png");
        img.save_with_format(&path, image::ImageFormat::Png)
            .unwrap();

        let on = process_file_args(&[path.to_string_lossy().into_owned()], dir.path(), true)
            .await
            .unwrap();
        assert!(
            on.text
                .contains("[Image: original 2600x800, displayed at 2000x"),
            "auto_resize=true must still downscale and annotate: {}",
            on.text
        );

        let off = process_file_args(&[path.to_string_lossy().into_owned()], dir.path(), false)
            .await
            .unwrap();
        assert!(
            !off.text.contains("displayed at"),
            "auto_resize=false must not resize: {}",
            off.text
        );
    }

    /// Build a PNG chunk: big-endian length, 4-byte type, payload, and a CRC placeholder (the
    /// sniffer reads neither the CRC nor the payload).
    fn png_chunk(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend((payload.len() as u32).to_be_bytes());
        out.extend(kind);
        out.extend(payload);
        out.extend([0u8; 4]);
        out
    }

    const PNG_SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

    async fn inline_one(dir: &std::path::Path, path: &std::path::Path) -> ProcessedFiles {
        process_file_args(&[path.to_string_lossy().into_owned()], dir, true)
            .await
            .expect("process")
    }

    /// SEAM-133 — the `@file` sniff is now the single faithful port in `cyrup-tools`
    /// (`detectSupportedImageMimeTypeFromFile`, file-processor.ts:49 @v0.87.1), so a file pi calls
    /// TEXT is inlined as text rather than shipped as a broken image.
    #[tokio::test]
    async fn non_images_pi_rejects_are_inlined_as_text() {
        let dir = tempfile::tempdir().unwrap();

        // (1) `BM` alone is not a bitmap: pi requires `isBmp` (mime.ts) — length >= 26, a sane
        // pixel-data offset, a 12 or 40..=124 DIB header, one colour plane, a known bit depth.
        let bmw = dir.path().join("f.txt");
        std::fs::write(&bmw, "BMW service log").unwrap();
        let out = inline_one(dir.path(), &bmw).await;
        assert!(out.images.is_empty(), "{:?}", out.images);
        assert_eq!(
            out.text,
            format!(
                "<file name=\"{}\">\nBMW service log\n</file>\n",
                bmw.display()
            )
        );

        // (2) an animated PNG (`acTL` before `IDAT`) — pi returns null for it (mime.ts:42-55).
        let apng = dir.path().join("a.png");
        let mut bytes = PNG_SIG.to_vec();
        bytes.extend(png_chunk(b"IHDR", &[0u8; 13]));
        bytes.extend(png_chunk(b"acTL", &[0u8; 8]));
        bytes.extend(png_chunk(b"IDAT", &[0u8; 4]));
        std::fs::write(&apng, &bytes).unwrap();
        let out = inline_one(dir.path(), &apng).await;
        assert!(out.images.is_empty(), "{:?}", out.images);
        assert!(out.text.contains("<file name="), "{}", out.text);

        // (3) lossless JPEG: `buffer[3] === 0xf7` → null (mime.ts).
        let lossless = dir.path().join("l.jpg");
        std::fs::write(&lossless, [0xff, 0xd8, 0xff, 0xf7, 0x00, 0x01]).unwrap();
        let out = inline_one(dir.path(), &lossless).await;
        assert!(out.images.is_empty(), "{:?}", out.images);
        assert!(out.text.contains("<file name="), "{}", out.text);
    }

    /// SEAM-133 — pi's text branch reads `stripBom(readFile(path, "utf-8"))`
    /// (file-processor.ts:77 @v0.87.1), so a UTF-8 BOM never reaches the prompt.
    #[tokio::test]
    async fn a_utf8_bom_is_stripped_from_an_inlined_text_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bom.md");
        std::fs::write(&path, b"\xef\xbb\xbfhello").unwrap();
        let out = inline_one(dir.path(), &path).await;
        assert!(out.images.is_empty());
        assert!(
            !out.text.contains('\u{feff}'),
            "the BOM survived into the prompt: {:?}",
            out.text
        );
        assert_eq!(
            out.text,
            format!("<file name=\"{}\">\nhello\n</file>\n", path.display())
        );
    }

    /// Presence before absence: the change cannot pass by rejecting everything. A real minimal PNG
    /// (signature + a length-13 IHDR) and a `GIF89a` file are still sniffed as images.
    #[test]
    fn real_image_headers_are_still_recognised() {
        let mut png = PNG_SIG.to_vec();
        png.extend(png_chunk(b"IHDR", &[0u8; 13]));
        assert_eq!(
            cyrup_tools::ImageMime::from_file_head(&png).map(cyrup_tools::ImageMime::mime),
            Some("image/png")
        );
        assert_eq!(
            cyrup_tools::ImageMime::from_file_head(b"GIF89a....").map(cyrup_tools::ImageMime::mime),
            Some("image/gif")
        );
        assert_eq!(cyrup_tools::ImageMime::from_file_head(b"plain text"), None);
    }
}
