//! `find` — in-process fd-parity glob search (R-03-032…034, arch-03 §6.7). [CYRUP-DELTA]: uses
//! `ignore::WalkBuilder` + `globset` instead of an external `fd` binary.

use crate::config::FindOpts;
use crate::details::FindDetails;
use crate::ops::{FsOps, WalkOpts};
use crate::tools::globmatch::{to_posix, PatternMatcher};
use crate::truncate::{format_size, truncate_head, TruncOpts};
use crate::{error, path, ToolMeta};
use cyrup_core::{CancelToken, Content, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink};
use futures::StreamExt;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct FindInput {
    pattern: String,
    path: Option<String>,
    limit: Option<usize>,
}

pub struct FindTool {
    fs: Arc<dyn FsOps>,
    cwd: PathBuf,
    opts: FindOpts,
    params: serde_json::Value,
}

impl FindTool {
    pub fn new(fs: Arc<dyn FsOps>, cwd: PathBuf, opts: FindOpts) -> Self {
        let params = serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Glob; basename match unless it contains '/'." },
                "path": { "type": "string", "description": "Search root (default '.')." },
                "limit": { "type": "integer", "minimum": 1, "description": "Max results (default 1000)." }
            },
            "required": ["pattern"],
            "additionalProperties": false
        });
        Self { fs, cwd, opts, params }
    }
}

#[async_trait::async_trait]
impl Tool for FindTool {
    fn name(&self) -> &str {
        "find"
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
        let input: FindInput =
            serde_json::from_value(params).map_err(|e| error::invalid(format!("find: {e}")))?;

        let search_root = path::resolve_to_cwd(input.path.as_deref().unwrap_or("."), &self.cwd);
        self.fs
            .metadata(&search_root)
            .await
            .map_err(|_| error::not_found(format!("Search path not found: {}", error::show(&search_root))))?;

        let matcher = PatternMatcher::build(&input.pattern)?;
        let limit = input.limit.unwrap_or(self.opts.limit);

        let mut results: Vec<String> = Vec::new();
        let mut walk = self.fs.walk(&search_root, WalkOpts::default());
        loop {
            tokio::select! {
                _ = cancel.cancelled() => return Err(error::aborted()),
                item = walk.next() => {
                    match item {
                        Some(Ok(w)) => {
                            if w.path == search_root {
                                continue;
                            }
                            let rel_path = w.path.strip_prefix(&search_root).unwrap_or(&w.path);
                            let rel = to_posix(rel_path);
                            let basename = w.path
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| rel.clone());
                            if matcher.is_match(&rel, &basename) {
                                let entry = if w.is_dir { format!("{rel}/") } else { rel };
                                results.push(entry);
                            }
                        }
                        Some(Err(e)) => return Err(e),
                        None => break,
                    }
                }
            }
        }

        if results.is_empty() {
            return Ok(ToolResult {
                content: vec![Content::text("No files found matching pattern")],
                details: None,
                terminate: false,
            });
        }

        results.sort();
        let total = results.len();
        let limit_reached = total > limit;
        if limit_reached {
            results.truncate(limit);
        }

        let joined = results.join("\n");
        let t = truncate_head(&joined, TruncOpts::bytes_only(self.opts.max_bytes));

        let mut text = t.content.clone();
        if limit_reached {
            text.push_str(&format!(
                "\n\n[{limit} result limit reached. Use limit={} to see more.]",
                limit.saturating_mul(2)
            ));
        }
        if t.info.truncated {
            text.push_str(&format!("\n\n[Output truncated at {}.]", format_size(self.opts.max_bytes)));
        }

        let details = FindDetails {
            truncation: Some(t.info),
            result_limit_reached: if limit_reached { Some(limit) } else { None },
        };

        Ok(ToolResult {
            content: vec![Content::text(text)],
            details: serde_json::to_value(details).ok(),
            terminate: false,
        })
    }
}

impl ToolMeta for FindTool {
    fn description(&self) -> &str {
        "Find files by glob (gitignore-aware), returning sorted relative paths."
    }
    fn prompt_snippet(&self) -> Option<&str> {
        Some("find: locate files by glob pattern.")
    }
    fn prompt_guidelines(&self) -> &[&str] {
        &["Use `find` to locate files by name; a pattern with '/' matches the full relative path."]
    }
}
