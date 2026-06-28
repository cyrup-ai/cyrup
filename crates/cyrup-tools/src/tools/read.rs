//! `read` — text window + image attachment (R-03-011…014, arch-03 §6.3). One-shot, no streaming.

use crate::config::ReadOpts;
use crate::details::ReadDetails;
use crate::ops::FsOps;
use crate::truncate::{format_size, truncate_head, TruncOpts};
use crate::{error, path, ToolMeta};
use cyrup_core::{CancelToken, Content, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadInput {
    path: String,
    offset: Option<usize>,
    limit: Option<usize>,
}

pub struct ReadTool {
    fs: Arc<dyn FsOps>,
    cwd: PathBuf,
    opts: ReadOpts,
    params: serde_json::Value,
}

impl ReadTool {
    pub fn new(fs: Arc<dyn FsOps>, cwd: PathBuf, opts: ReadOpts) -> Self {
        let params = serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path (relative to cwd or absolute)." },
                "offset": { "type": "integer", "minimum": 1, "description": "1-indexed start line." },
                "limit": { "type": "integer", "minimum": 1, "description": "Max lines to read." }
            },
            "required": ["path"],
            "additionalProperties": false
        });
        Self { fs, cwd, opts, params }
    }
}

#[async_trait::async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str {
        "read"
    }
    fn parameters(&self) -> &serde_json::Value {
        &self.params
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

        // Resolve to an existing candidate (macOS variant fallback, R-03-006).
        let candidates = path::resolve_read_path(&input.path, &self.cwd);
        let mut abs = None;
        for cand in &candidates {
            if self.fs.access(cand, crate::ops::Access::Read).await.is_ok() {
                abs = Some(cand.clone());
                break;
            }
        }
        let abs = abs.ok_or_else(|| {
            error::not_found(format!("File not found or unreadable: {}", input.path))
        })?;

        if cancel.is_cancelled() {
            return Err(error::aborted());
        }

        // Image branch (R-03-012).
        if let Some(mime) = self.fs.detect_image_mime(&abs) {
            return self.read_image(&abs, mime).await;
        }

        // Text branch (R-03-011).
        let bytes = self.fs.read(&abs).await?;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let lines: Vec<&str> = text.split('\n').collect();
        let total = if text.ends_with('\n') { lines.len().saturating_sub(1) } else { lines.len() };

        let start = input.offset.map(|o| o.saturating_sub(1)).unwrap_or(0);
        if start >= total && total > 0 {
            return Err(error::invalid(format!(
                "Offset {} is beyond end of file ({} lines)",
                input.offset.unwrap_or(1),
                total
            )));
        }

        let end = match input.limit {
            Some(l) => (start + l).min(total),
            None => total,
        };
        let window: Vec<&str> =
            lines.get(start..end).map(<[&str]>::to_vec).unwrap_or_default();
        let window_text = window.join("\n");

        let t = truncate_head(
            &window_text,
            TruncOpts::new(self.opts.max_lines, self.opts.max_bytes),
        );

        if t.info.first_line_exceeds_limit {
            let line_no = start + 1;
            return Err(error::invalid(format!(
                "[Line {line_no} is {}, exceeds the {} read limit. Use bash: sed -n '{line_no}p' {} | head -c {}]",
                format_size(t.info.total_bytes),
                format_size(self.opts.max_bytes),
                input.path,
                self.opts.max_bytes,
            )));
        }

        let mut out = t.content.clone();
        if t.info.truncated {
            let shown_to = start + t.info.output_lines;
            out.push_str(&format!(
                "\n\n[Showing lines {}-{} of {}. Use offset={} to continue.]",
                start + 1,
                shown_to,
                total,
                shown_to + 1
            ));
        } else if end < total {
            let remaining = total - end;
            out.push_str(&format!(
                "\n\n[{remaining} more lines in file. Use offset={} to continue.]",
                end + 1
            ));
        }

        Ok(ToolResult {
            content: vec![Content::text(out)],
            details: serde_json::to_value(ReadDetails { truncation: Some(t.info) }).ok(),
            terminate: false,
        })
    }
}

impl ReadTool {
    async fn read_image(
        &self,
        abs: &std::path::Path,
        mime: crate::ops::ImageMime,
    ) -> Result<ToolResult, ToolError> {
        let bytes = self.fs.read(abs).await?;

        if !self.opts.supports_images {
            let note = format!(
                "Read image file [{}] ({}).\n[Current model does not support images; image data omitted.]",
                mime.mime(),
                format_size(bytes.len())
            );
            return Ok(ToolResult {
                content: vec![Content::text(note)],
                details: None,
                terminate: false,
            });
        }

        #[cfg(feature = "inline-images")]
        {
            match encode_image(&bytes, self.opts.max_image_dim) {
                Ok((data, out_mime)) => {
                    let note = format!("Read image file [{}].", mime.mime());
                    Ok(ToolResult {
                        content: vec![
                            Content::text(note),
                            Content::Image { data, mime_type: out_mime },
                        ],
                        details: None,
                        terminate: false,
                    })
                }
                Err(e) => Err(error::invalid(format!("Could not decode image: {e}"))),
            }
        }

        #[cfg(not(feature = "inline-images"))]
        {
            let note = format!(
                "Read image file [{}] ({}).\n[Image inlining is not enabled in this build (feature `inline-images`).]",
                mime.mime(),
                format_size(bytes.len())
            );
            Ok(ToolResult {
                content: vec![Content::text(note)],
                details: None,
                terminate: false,
            })
        }
    }
}

#[cfg(feature = "inline-images")]
fn encode_image(bytes: &[u8], max_dim: u32) -> Result<(String, String), String> {
    use std::io::Cursor;
    let img = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
    let (w, h) = (img.width(), img.height());
    let resized = if w > max_dim || h > max_dim {
        img.resize(max_dim, max_dim, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let mut out = Cursor::new(Vec::new());
    resized
        .write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok((base64_encode(&out.into_inner()), "image/png".to_string()))
}

/// Minimal RFC 4648 base64 (no padding omission); avoids an extra dependency.
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

impl ToolMeta for ReadTool {
    fn description(&self) -> &str {
        "Read a text file (UTF-8) within a line/byte window, or return an image attachment."
    }
    fn prompt_snippet(&self) -> Option<&str> {
        Some("read: read a file's contents (text window or image).")
    }
    fn prompt_guidelines(&self) -> &[&str] {
        &["Use `read` to inspect file contents before editing; pass `offset`/`limit` for large files."]
    }
}
