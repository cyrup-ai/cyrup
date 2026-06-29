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
        // Pi runs `fd --hidden` (find.ts:224): match dotfiles/dot-dirs while still honoring
        // `.gitignore` (arch-03:430). So include hidden files in the walk.
        let mut walk = self.fs.walk(&search_root, WalkOpts { include_hidden: true });
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
        if results.len() > limit {
            results.truncate(limit);
        }
        // Pi: `resultLimitReached = relativized.length >= effectiveLimit` (find.ts:322).
        let limit_reached = results.len() >= limit;

        let joined = results.join("\n");
        let t = truncate_head(&joined, TruncOpts::bytes_only(self.opts.max_bytes));

        // Pi joins notices into ONE bracket (find.ts:327-339).
        let mut text = t.content.clone();
        let mut notices: Vec<String> = Vec::new();
        if limit_reached {
            notices.push(format!(
                "{limit} results limit reached. Use limit={} for more, or refine pattern",
                limit.saturating_mul(2)
            ));
        }
        if t.info.truncated {
            notices.push(format!("{} limit reached", format_size(self.opts.max_bytes)));
        }
        if !notices.is_empty() {
            text.push_str(&format!("\n\n[{}]", notices.join(". ")));
        }

        // Pi adds `truncation` only when the byte cap fired and emits `details: undefined` when no
        // key is set (find.ts:326-344). Mirror that exactly.
        let result_limit_reached = if limit_reached { Some(limit) } else { None };
        let truncation = if t.info.truncated { Some(t.info) } else { None };
        let details = if truncation.is_some() || result_limit_reached.is_some() {
            serde_json::to_value(FindDetails { truncation, result_limit_reached }).ok()
        } else {
            None
        };

        Ok(ToolResult {
            content: vec![Content::text(text)],
            details,
            terminate: false,
        })
    }
}

impl ToolMeta for FindTool {
    // Verbatim from Pi (find.ts:117-118). DEFAULT_LIMIT=1000, DEFAULT_MAX_BYTES/1024=50. Pi defines
    // no promptGuidelines for find.
    fn description(&self) -> &str {
        "Search for files by glob pattern. Returns matching file paths relative to the search \
         directory. Respects .gitignore. Output is truncated to 1000 results or 50KB (whichever is \
         hit first)."
    }
    fn prompt_snippet(&self) -> Option<&str> {
        Some("Find files by glob pattern (respects .gitignore)")
    }
}
