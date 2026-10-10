//! `read` — text window + image attachment (R-03-011…014, arch-03 §6.3). One-shot, no streaming.

use crate::config::ReadOpts;
use crate::details::ReadDetails;
use crate::ops::FsOps;
use crate::ops::cancel_read::{Cancelled, read_to_end_cancellable};
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

/// The largest file `read` takes into memory: V8's maximum string length, `0x1fffffe8` (536,870,888)
/// UTF-16 code units.
///
/// Pi's text branch is `buffer.toString("utf-8")` (read.ts, `const textContent` @v1.0.4), and Node
/// 22 refuses to build a string past that length with `ERR_STRING_TOO_LONG` ("Cannot create a
/// string longer than 0x1fffffe8 characters"; checked against Node 22.22.2 with a 600 MB file), so
/// no larger ASCII text file has ever been readable through Pi's `read`. cyrup keeps the whole file as bytes while it windows it
/// and has no such ceiling of its own, so one `read` of a multi-gigabyte file, or of a device that
/// never ends, grew the host until it ran out of memory (the sandbox's limits bound the script, not
/// the host work its tool calls trigger). Judged in bytes, because the size is known before the
/// text is decoded.
///
/// [CYRUP-DELTA] Pi's image branch has no string to build and reads a file of up to 2 GiB; here an
/// image over this limit is refused as well. A 512 MiB image is not a screenshot or a photo any
/// model accepts, and the decision between text and image is made on the bytes, after the read.
pub(crate) const MAX_READ_BYTES: u64 = 0x1fff_ffe8;

/// Node's `kIoMaxLength` (`2 ** 31 - 1`): `fs.readFile` rejects a regular file over this size with
/// `ERR_FS_FILE_TOO_LARGE`, whose message is `File size (<bytes>) is greater than 2 GiB` (checked
/// against Node 22: a 2 GiB sparse file is refused, one byte less is read).
const NODE_IO_MAX_LENGTH: u64 = i32::MAX as u64;

pub struct ReadTool {
    fs: Arc<dyn FsOps>,
    cwd: PathBuf,
    opts: ReadOpts,
    params: serde_json::Value,
    /// Pi's `readOutputSchema` (`read.ts:33-36` @v1.1.0).
    output_schema: serde_json::Value,
    /// The byte ceiling [`MAX_READ_BYTES`], a field so a test can pin the check with a small file.
    max_read_bytes: u64,
}

/// Pi's `toReadOutput` (`read.ts:72-77` @v1.1.0): the first text block's text, or `""`; with an
/// image block, that block and the text as its `note`.
fn to_read_output(content: &[Content]) -> serde_json::Value {
    let text = content
        .iter()
        .find_map(|block| match block {
            Content::Text { text, .. } => Some(text.to_string()),
            _ => None,
        })
        .unwrap_or_default();
    let image = content.iter().find_map(|block| match block {
        Content::Image { data, mime_type } => Some((data, mime_type)),
        _ => None,
    });
    match image {
        Some((data, mime_type)) => serde_json::json!({
            "type": "image",
            "data": data,
            "mimeType": mime_type,
            "note": text,
        }),
        None => serde_json::Value::String(text),
    }
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
        // `readOutputSchema` as TypeBox 1.3.27 emits it (read.ts:27-36 @v1.1.0): the text, or the
        // image block codemode's `image()` accepts with the note that goes with it. `Type.Literal`
        // emits `type` beside `const`, and upstream leaves out property descriptions on purpose so
        // the declared type stays on one line.
        let output_schema = serde_json::json!({
            "anyOf": [
                { "type": "string" },
                {
                    "type": "object",
                    "required": ["type", "data", "mimeType", "note"],
                    "properties": {
                        "type": { "type": "string", "const": "image" },
                        "data": { "type": "string" },
                        "mimeType": { "type": "string" },
                        "note": { "type": "string" }
                    }
                }
            ]
        });
        Self {
            fs,
            cwd,
            opts,
            params,
            output_schema,
            max_read_bytes: MAX_READ_BYTES,
        }
    }

    /// A tool that refuses files over `max_read_bytes` instead of [`MAX_READ_BYTES`].
    #[cfg(test)]
    pub(crate) fn with_max_read_bytes(mut self, max_read_bytes: u64) -> Self {
        self.max_read_bytes = max_read_bytes;
        self
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

    /// TOOL-058 — `outputSchema: readOutputSchema` (`read.ts:100` @v1.1.0), so a codemode script's
    /// `tools.read` resolves to the text, or to an image block its `image()` shows.
    fn output_schema(&self) -> Option<&serde_json::Value> {
        Some(&self.output_schema)
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

    /// Pi `constrainedSampling: { type: "json_schema", strict: "prefer" }`
    /// (`core/tools/read.ts:80` @v0.87.1) — declared unconditionally since pi 0.86.0, which
    /// removed the `PI_EXPERIMENTAL` gate. This asks a strict-capable route to constrain
    /// generation to the declared schema; `prefer` degrades silently elsewhere.
    fn constrained_sampling(&self) -> Option<&cyrup_core::ConstrainedSampling> {
        crate::tools::prefer_strict_tool_sampling()
    }

    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: serde_json::Value,
        cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        // TOOL-058 — pi's `.then((result) => ({ ...result, structuredContent: toReadOutput(result.content) }))`
        // (`read.ts:216` @v1.1.0): EVERY resolved read carries the structured half, whichever branch
        // produced it, and a failure rejects before it exists.
        let mut result = self.read_file(params, cancel).await?;
        result.structured_content = Some(to_read_output(&result.content));
        Ok(result)
    }
}

impl ReadTool {
    /// The read itself; [`Tool::execute`] adds the structured half to whatever this resolves.
    async fn read_file(
        &self,
        params: serde_json::Value,
        cancel: CancelToken,
    ) -> Result<ToolResult, ToolError> {
        // Pi's FIRST statement inside the promise body (read.ts:232-235), ahead of
        // `resolveReadPathAsync` and ahead of any argument handling — Pi never validates tool
        // arguments, so an already-fired signal cannot lose to a schema error there. Hence before
        // `from_value` here too. Without this guard the tool ran the entire macOS-variant
        // `access(Exists)` probe loop below, plus the `R_OK` check, on a run the user had already
        // cancelled — on a remote `FsOps` that is several round-trips of pure waste.
        if cancel.is_cancelled() {
            return Err(error::aborted());
        }

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
        // `"{resolved path}: {io error}"` (ops/local/fs.rs), so propagating is enough.

        // Pi's guard between `resolveReadPathAsync` and `ops.access` (read.ts:246). The resolution
        // loop above is `candidates.len()` round-trips through the seam — one per macOS filename
        // variant — so on a remote backend it is real latency the token must be able to cut. No
        // per-candidate check is added INSIDE that loop: Pi's probes live inside
        // `resolveReadPathAsync` and are equally unguarded, and this is a parity task.
        if cancel.is_cancelled() {
            return Err(error::aborted());
        }

        self.fs.access(&abs, crate::ops::Access::Read).await?;

        // Pi's guard between `ops.access` and `ops.readFile` (read.ts:249).
        if cancel.is_cancelled() {
            return Err(error::aborted());
        }

        // What `ops.readFile` would refuse or never finish, refused before a byte is read.
        self.check_readable(&abs, &cancel).await?;

        // Read through the (remote-aware) seam, then decide text-vs-image by MAGIC BYTES — Pi
        // sniffs the file header (read.ts:243 → mime.ts), not the extension.
        let bytes = self.read_cancellable(&abs, &cancel).await?;

        // Image branch (R-03-012). The sniff sees only the first `IMAGE_TYPE_SNIFF_BYTES` (4100)
        // bytes, which is the window Pi's `detectSupportedImageMimeTypeFromFile` reads
        // (mime.ts:28-30) before calling the same predicate — NOT the whole file. Handing the
        // whole file to the sniffer let `isAnimatedPng`'s chunk walk (mime.ts:42-55) find an
        // `acTL` past byte 4100, where Pi's walk has already bailed at `:51`; the file then fell
        // through to the TEXT branch below and the model got `from_utf8_lossy` of a PNG instead of
        // the picture Pi shows.
        if let Some(mime) = crate::ops::ImageMime::from_file_head(&bytes) {
            let result = self.read_image(bytes, mime, &cancel).await?;
            // Pi's single common guard before `resolve` (read.ts:325) dominates BOTH image result
            // shapes — `ok` and `failed` — so a cancel landing during the decode/resize ladder
            // yields the abort, never an image block.
            if cancel.is_cancelled() {
                return Err(error::aborted());
            }
            return Ok(result);
        }

        // Text branch (R-03-011). The bytes become the string without a copy when they are valid
        // UTF-8, so a file near [`MAX_READ_BYTES`] is held once here, not twice.
        let text = match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
        };
        // Pi's basis is `allLines.length` — the raw `split("\n")` count, which INCLUDES the empty
        // phantom element after a trailing newline (read.ts:268-269). Do not pop it: the offset
        // bound, the `of N` continuation count, and the out-of-bounds error all key off this count.
        // Counted rather than collected: a `Vec<&str>` of every line costs 16 bytes per line, which
        // for a file of blank lines is sixteen times the file.
        let total = text.bytes().filter(|&b| b == b'\n').count() + 1;

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
        // `allLines.slice(start, end).join("\n")`, as the slice of `text` it is.
        let window_text = lines_window(&text, start, end, total);

        let t = truncate_head(
            window_text,
            TruncOpts::new(self.opts.max_lines, self.opts.max_bytes),
        );

        if t.info.first_line_exceeds_limit {
            // Pi resolves SUCCESSFULLY here (read.ts:290-294,315): the note is the content and the
            // truncation is attached as `details`, so the model gets an actionable result, not an
            // `isError` failure. `firstLineSize` is the byte length of the first selected line.
            let line_no = start + 1;
            let first_line_bytes = window_text.split('\n').next().map_or(0, str::len);
            // Pi hardcodes `formatSize(DEFAULT_MAX_BYTES)` and `head -c ${DEFAULT_MAX_BYTES}` here
            // (read.ts:293), independent of any configured limit. Use the fixed constant.
            let out = format!(
                "[Line {line_no} is {}, exceeds {} limit. Use bash: sed -n '{line_no}p' {} | head -c {}]",
                format_size(first_line_bytes),
                format_size(DEFAULT_MAX_BYTES),
                input.path,
                DEFAULT_MAX_BYTES,
            );
            // Pi's common pre-`resolve` guard (read.ts:325) covers this shape too — the
            // `firstLineExceedsLimit` branch assigns `outputText` and falls THROUGH to it.
            if cancel.is_cancelled() {
                return Err(error::aborted());
            }
            return Ok(ToolResult {
                content: vec![Content::text(out)],
                details: serde_json::to_value(ReadDetails {
                    truncation: Some(t.info),
                })
                .ok(),
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
        // Pi's `if (aborted) return;` immediately before `resolve` (read.ts:325). The `split('\n')`,
        // the window slice, `truncate_head` and the continuation-notice formatting above are all
        // CPU work over a file that may be tens of megabytes, so this is a real window, not a
        // formality.
        if cancel.is_cancelled() {
            return Err(error::aborted());
        }
        Ok(ToolResult {
            content: vec![Content::text(out)],
            details,
            ..Default::default()
        })
    }
}

impl ReadTool {
    /// Refuse, before a byte is read, what `ops.readFile` would refuse or never finish.
    ///
    /// **Regular files over [`Self::max_read_bytes`].** The size comes from the backend's
    /// `metadata`. Over 2 GiB the message is Node's own, `File size (<bytes>) is greater than
    /// 2 GiB`, which `fs.readFile` raises as `ERR_FS_FILE_TOO_LARGE`.
    ///
    /// **Anything that is neither a regular file nor a directory** (a character or block device, a
    /// FIFO, a socket). [CYRUP-DELTA] Node's `readFile` opens `/dev/zero` and reads until the
    /// process runs out of memory, and opens a FIFO and waits for a writer; here the model gets a
    /// one-line error instead of a host that grows by gigabytes a second (the memory limits of a
    /// codemode sandbox bound the script, not the host work its `tools.read` calls trigger). A
    /// directory is let through so it still fails the way it always has, at the read.
    ///
    /// A backend that cannot answer `metadata` (an RPC filesystem without it) is not an error
    /// here: `ops.access` already vouched for the path, and the drain in
    /// [`Self::read_cancellable`] enforces the same byte limit on whatever arrives.
    async fn check_readable(
        &self,
        abs: &std::path::Path,
        cancel: &CancelToken,
    ) -> Result<(), ToolError> {
        let Some(stat) = cancel.run_until_cancelled(self.fs.metadata(abs)).await else {
            return Err(error::aborted());
        };
        let Ok(meta) = stat else {
            return Ok(());
        };
        if !meta.is_file && !meta.is_dir {
            return Err(error::invalid(format!(
                "Cannot read {}: it is not a regular file (a device, pipe or socket)",
                error::show(abs)
            )));
        }
        if meta.is_file && meta.len > self.max_read_bytes {
            return Err(self.too_large(Some(meta.len)));
        }
        Ok(())
    }

    /// The error for a file past the limit: `len` is its size when `stat` said, `None` when the
    /// stream outran the limit without ever announcing a size.
    fn too_large(&self, len: Option<u64>) -> ToolError {
        match len {
            // `ERR_FS_FILE_TOO_LARGE`, verbatim (`File size (%s) is greater than 2 GiB`).
            Some(len) if len > NODE_IO_MAX_LENGTH => {
                error::invalid(format!("File size ({len}) is greater than 2 GiB"))
            }
            _ => {
                let limit = format_size(usize::try_from(self.max_read_bytes).unwrap_or(usize::MAX));
                let size = len.map_or_else(String::new, |len| format!(" ({len})"));
                error::invalid(format!(
                    "File size{size} is greater than the {limit} limit for read. \
                     Use bash (head, tail, sed -n or grep) to look at part of it."
                ))
            }
        }
    }

    /// Faithful port of Pi's image read path (read.ts:247-263). The model-facing note is
    /// `Read image file [<mime>]` plus any processing hints, and — for non-vision models — the
    /// image block is STILL returned together with a warning note (Pi keeps the block; the request
    /// layer strips it later). `mime` is the magic-byte-detected type.
    /// `ops.readFile` (read.ts:256 / :273) with Pi's abort listener attached.
    ///
    /// Two independent windows have to be covered, and they need different mechanisms.
    ///
    /// **Opening the stream** is where a backend that cannot stream does its ENTIRE transfer:
    /// [`FsOps::read_stream`]'s default body is `Cursor::new(self.read(path).await?)`, so for a
    /// remote/RPC `FsOps` the whole file arrives inside this one `await`. Nothing inside it can be
    /// interrupted, so it is RACED and the orphaned future is dropped — precisely Pi's shape, where
    /// the listener rejects the promise while libuv's read is still in flight and nobody cancels
    /// libuv either.
    ///
    /// **Draining the stream** is where a large local file spends its time, and there the work
    /// itself must stop rather than merely be abandoned: `spawn_blocking` tasks cannot be aborted
    /// and dropping the `JoinHandle` only detaches, so a bare race would leave a thread reading
    /// gigabytes into a `Vec` nobody will read. [`read_to_end_cancellable`] carries the token
    /// INSIDE the blocking closure and unwinds within one 64 KiB buffer fill. The outer race is
    /// still required: it is what makes the CALLER return at once when a single fill is parked on
    /// a slow device.
    ///
    /// `run_until_cancelled` rather than a `biased` `select!`: it short-circuits to `None` when the
    /// token is ALREADY cancelled (tokio-util 0.7.18 `sync/cancellation_token.rs:280-293`), which
    /// is the same determinism `find.rs` gets from `biased;`, and it is the idiom the `grep`
    /// sibling uses for the identical races.
    ///
    /// The seam moves from [`FsOps::read`] to [`FsOps::read_stream`], and that is safe for every
    /// backend: `LocalFs` overrides it with a real `File` (ops/local/fs.rs), both isolation
    /// decorators forward it explicitly (protected.rs, traversal.rs — the latter re-applying
    /// `confine` to the forwarded path), and any implementation that overrides only `read` inherits
    /// the default, which routes straight back through that implementation's own `read` — so
    /// recording/counting backends still see this tool's traffic.
    async fn read_cancellable(
        &self,
        abs: &std::path::Path,
        cancel: &CancelToken,
    ) -> Result<Vec<u8>, ToolError> {
        let Some(opened) = cancel.run_until_cancelled(self.fs.read_stream(abs)).await else {
            return Err(error::aborted());
        };
        // A failure to OPEN keeps propagating uncaught, exactly as `self.fs.read(&abs)?` did and as
        // Pi's uncaught `ops.readFile` rejection does (read.ts:321-324).
        let reader = opened?;

        let token = cancel.clone();
        // A limit that does not fit `usize` (a 32-bit host) is no limit the address space could
        // reach anyway.
        let limit = usize::try_from(self.max_read_bytes).unwrap_or(usize::MAX);
        let drain =
            tokio::task::spawn_blocking(move || read_to_end_cancellable(reader, &token, limit));

        // Dropping `drain` here DETACHES the blocking task rather than killing it; that is
        // acceptable only because the closure holds the same token and returns within one buffer
        // fill. Never weaken `read_to_end_cancellable` to rely on this race alone.
        let Some(joined) = cancel.run_until_cancelled(drain).await else {
            return Err(error::aborted());
        };

        match joined {
            Ok(Ok(bytes)) => Ok(bytes),
            // The token fired between two buffer fills: report Pi's abort, not a raw I/O error.
            Ok(Err(e)) if Cancelled::is(&e) => Err(error::aborted()),
            // A stream that was longer than `stat` said, or whose length `stat` never knew. The
            // buffer stopped at the limit; the rest of the file was not read.
            Ok(Err(e)) if e.kind() == std::io::ErrorKind::FileTooLarge => Err(self.too_large(None)),
            // A genuine backend failure keeps the shape `LocalFs::read` produced before this
            // change: `"{resolved path}: {io error}"`.
            Ok(Err(e)) => Err(error::io(&error::show(abs), &e)),
            // The blocking task panicked or was cancelled by runtime shutdown.
            Err(e) => Err(error::invalid(format!("read: {e}"))),
        }
    }

    async fn read_image(
        &self,
        bytes: Vec<u8>,
        mime: crate::ops::ImageMime,
        cancel: &CancelToken,
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
            // Pi `await`s `processImage` with the abort listener live (read.ts:257), so an abort
            // during it rejects at once. Here it is a SYNCHRONOUS decode + EXIF + resize +
            // JPEG-quality ladder invoked inline on the async worker: it observed no token AND
            // pinned a runtime thread for the whole ladder. Moving it onto the blocking pool is
            // what makes the race expressible at all. `Processed` is owned (`String` /
            // `Vec<String>`), so it crosses the boundary unchanged; `ImageMime` is `Copy` and the
            // two option fields are scalars, so nothing borrows `self` across the spawn.
            // The profile is resolved to an OWNED value here, before the spawn: Pi reads
            // `ctx?.model?.inputLimits?.images?.resize` per call (read.ts:138), and cloning out of
            // the options is what keeps the resolve on the async side while the decode runs on the
            // blocking pool with nothing borrowed from `self`.
            let resize = self.opts.resize_profile_now();
            let auto_resize = self.opts.auto_resize_images;
            let mime_str = mime.mime();
            let processing = tokio::task::spawn_blocking(move || {
                crate::image_proc::process_image(
                    &bytes,
                    mime_str,
                    crate::image_proc::ProcessImageOptions {
                        auto_resize_images: auto_resize,
                        resize: resize.as_ref(),
                    },
                )
            });
            let Some(joined) = cancel.run_until_cancelled(processing).await else {
                return Err(error::aborted());
            };
            let processed = joined.map_err(|e| error::invalid(format!("read: {e}")))?;
            match processed {
                crate::image_proc::Processed::Ok {
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
                        ..Default::default()
                    })
                }
                crate::image_proc::Processed::Failed { message } => {
                    // `Read image file [${mimeType}]\n${message}` + nonVisionNote (no image block).
                    let mut note = format!("Read image file [{}]\n{message}", mime.mime());
                    if let Some(nv) = non_vision_note {
                        note.push('\n');
                        note.push_str(nv);
                    }
                    Ok(ToolResult {
                        content: vec![Content::text(note)],
                        details: None,
                        ..Default::default()
                    })
                }
            }
        }

        #[cfg(not(feature = "inline-images"))]
        {
            // No decode happens in this build, so there is no work to race — but Pi's guard before
            // `resolve` (read.ts:325) still applies, and this keeps `cancel` used on BOTH cfg arms
            // without an `allow` attribute or an underscore-renamed parameter.
            if cancel.is_cancelled() {
                return Err(error::aborted());
            }
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
                ..Default::default()
            })
        }
    }
}

/// `text.split('\n').collect::<Vec<_>>()[start..end].join("\n")` (Pi's
/// `allLines.slice(startLine, endLine).join("\n")`, read.ts), as the slice of `text` it is, so the
/// window costs no allocation however many lines `text` has. `total` is the number of `split('\n')`
/// items, and `start <= end <= total`.
///
/// The join of consecutive `split('\n')` items is exactly the bytes between the newline that opens
/// line `start` and the newline that closes line `end - 1`; both are ASCII, so both offsets fall on
/// character boundaries.
fn lines_window(text: &str, start: usize, end: usize, total: usize) -> &str {
    if end <= start {
        return "";
    }
    // Where line `n` begins: just past the `n`-th newline.
    let line_start = |n: usize| match n.checked_sub(1) {
        None => 0,
        Some(newlines_before) => text
            .match_indices('\n')
            .nth(newlines_before)
            .map_or(text.len(), |(at, _)| at + 1),
    };
    let from = line_start(start);
    // The window runs to the end of the text when it takes the last line, else up to (not
    // including) the newline that precedes line `end`.
    let to = if end >= total {
        text.len()
    } else {
        line_start(end).saturating_sub(1)
    };
    text.get(from..to).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{ReadTool, lines_window};
    use crate::config::ReadOpts;
    use crate::ops::local::LocalFs;
    use cyrup_core::{ConstrainedSampling, ConstrainedSamplingConfig, StrictSampling, Tool};
    use std::path::PathBuf;
    use std::sync::Arc;
    /// TOOL-046 — the declaration is UNCONDITIONAL: pi 0.86.0 removed the `PI_EXPERIMENTAL` gate
    /// and `core/tools/read.ts:80` @v0.87.1 states the literal
    /// `constrainedSampling: { type: "json_schema", strict: "prefer" }`. No env is read here,
    /// because none is read upstream.
    #[test]
    fn read_declares_strict_prefer_constrained_sampling_unconditionally() {
        let tool = ReadTool::new(Arc::new(LocalFs), PathBuf::from("."), ReadOpts::default());
        assert_eq!(
            tool.constrained_sampling(),
            Some(&ConstrainedSampling::Config(
                ConstrainedSamplingConfig::JsonSchema {
                    strict: StrictSampling::Prefer
                }
            ))
        );
    }

    /// The text window is the slice of the file `split('\n')` + `slice` + `join('\n')` produces,
    /// for every window of every shape of text: leading, trailing and doubled newlines, an empty
    /// text, a multibyte character next to a newline.
    #[test]
    fn the_window_is_what_split_slice_join_would_give() {
        let texts = [
            "",
            "\n",
            "\n\n\n",
            "a",
            "a\n",
            "\na",
            "a\nb",
            "a\nb\n",
            "a\n\nb",
            "\u{e9}\n\u{fc}\n",
            "one\ntwo\nthree\nfour",
        ];
        for text in texts {
            let lines: Vec<&str> = text.split('\n').collect();
            let total = lines.len();
            for start in 0..total {
                for end in start..=total {
                    let expected = lines.get(start..end).unwrap_or_default().join("\n");
                    assert_eq!(
                        lines_window(text, start, end, total),
                        expected,
                        "text {text:?} window {start}..{end} of {total}"
                    );
                }
            }
        }
    }
}
