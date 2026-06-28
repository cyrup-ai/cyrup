//! `grep` — in-process ripgrep-parity search (R-03-029…031, arch-03 §6.6). [CYRUP-DELTA]: uses the
//! `ignore`/`grep` crates instead of an external `rg` binary; output format, gitignore semantics,
//! and truncation preserve Pi's observable behavior.

use crate::config::GrepOpts;
use crate::ops::{FsOps, WalkOpts};
use crate::tools::globmatch::{to_posix, PatternMatcher};
use crate::truncate::{format_size, truncate_head, truncate_line, GREP_MAX_LINE_LENGTH, TruncOpts};
use crate::{error, path, ToolMeta};
use cyrup_core::{CancelToken, Content, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink};
use futures::StreamExt;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::{Searcher, SearcherBuilder, Sink, SinkContext, SinkMatch};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct GrepInput {
    pattern: String,
    path: Option<String>,
    glob: Option<String>,
    ignore_case: Option<bool>,
    literal: Option<bool>,
    context: Option<usize>,
    limit: Option<usize>,
}

pub struct GrepTool {
    fs: Arc<dyn FsOps>,
    cwd: PathBuf,
    opts: GrepOpts,
    params: serde_json::Value,
}

impl GrepTool {
    pub fn new(fs: Arc<dyn FsOps>, cwd: PathBuf, opts: GrepOpts) -> Self {
        let params = serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Regex (or literal) to search for." },
                "path": { "type": "string", "description": "Search root (default '.')." },
                "glob": { "type": "string", "description": "Restrict to files matching this glob." },
                "ignoreCase": { "type": "boolean", "description": "Case-insensitive (default false)." },
                "literal": { "type": "boolean", "description": "Treat pattern as a literal string." },
                "context": { "type": "integer", "minimum": 0, "description": "Context lines before/after." },
                "limit": { "type": "integer", "minimum": 1, "description": "Max matches (default 100)." }
            },
            "required": ["pattern"],
            "additionalProperties": false
        });
        Self { fs, cwd, opts, params }
    }
}

struct GrepSink<'a> {
    rel: String,
    out: &'a mut Vec<String>,
    count: &'a mut usize,
    limit: usize,
    any_line_truncated: &'a mut bool,
}

fn clean(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim_end_matches(['\r', '\n']).to_string()
}

impl Sink for GrepSink<'_> {
    type Error = std::io::Error;

    fn matched(&mut self, _s: &Searcher, m: &SinkMatch<'_>) -> Result<bool, std::io::Error> {
        let line_no = m.line_number().unwrap_or(0);
        let text = clean(m.bytes());
        let (capped, tr) = truncate_line(&text, GREP_MAX_LINE_LENGTH);
        if tr {
            *self.any_line_truncated = true;
        }
        self.out.push(format!("{}:{}: {}", self.rel, line_no, capped));
        *self.count += 1;
        Ok(*self.count < self.limit)
    }

    fn context(&mut self, _s: &Searcher, c: &SinkContext<'_>) -> Result<bool, std::io::Error> {
        let line_no = c.line_number().unwrap_or(0);
        let text = clean(c.bytes());
        let (capped, tr) = truncate_line(&text, GREP_MAX_LINE_LENGTH);
        if tr {
            *self.any_line_truncated = true;
        }
        self.out.push(format!("{}-{}- {}", self.rel, line_no, capped));
        Ok(true)
    }
}

#[async_trait::async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
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
        let input: GrepInput =
            serde_json::from_value(params).map_err(|e| error::invalid(format!("grep: {e}")))?;

        let search_root = path::resolve_to_cwd(input.path.as_deref().unwrap_or("."), &self.cwd);
        let meta = self
            .fs
            .metadata(&search_root)
            .await
            .map_err(|_| error::not_found(format!("Search path not found: {}", error::show(&search_root))))?;

        let matcher = RegexMatcherBuilder::new()
            .case_insensitive(input.ignore_case.unwrap_or(false))
            .fixed_strings(input.literal.unwrap_or(false))
            .build(&input.pattern)
            .map_err(|e| error::invalid(format!("grep: invalid pattern: {e}")))?;

        let context = input.context.unwrap_or(0);
        let limit = input.limit.unwrap_or(self.opts.limit);

        let glob = match input.glob.as_deref() {
            Some(g) => Some(PatternMatcher::build(g)?),
            None => None,
        };

        // Collect candidate files (sorted for stable output).
        let mut files: Vec<(PathBuf, String)> = Vec::new();
        if meta.is_file {
            let rel = search_root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| to_posix(&search_root));
            files.push((search_root.clone(), rel));
        } else {
            let mut walk = self.fs.walk(&search_root, WalkOpts::default());
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => return Err(error::aborted()),
                    item = walk.next() => {
                        match item {
                            Some(Ok(w)) if !w.is_dir => {
                                let rel_path = w.path.strip_prefix(&search_root).unwrap_or(&w.path);
                                let rel = to_posix(rel_path);
                                let basename = w.path
                                    .file_name()
                                    .map(|n| n.to_string_lossy().into_owned())
                                    .unwrap_or_else(|| rel.clone());
                                if let Some(g) = &glob {
                                    if !g.is_match(&rel, &basename) {
                                        continue;
                                    }
                                }
                                files.push((w.path, rel));
                            }
                            Some(Ok(_)) => {}
                            Some(Err(e)) => return Err(e),
                            None => break,
                        }
                    }
                }
            }
            files.sort_by(|a, b| a.1.cmp(&b.1));
        }

        let mut searcher: Searcher = SearcherBuilder::new()
            .line_number(true)
            .before_context(context)
            .after_context(context)
            .build();

        let mut out: Vec<String> = Vec::new();
        let mut count = 0usize;
        let mut any_line_truncated = false;

        for (file, rel) in &files {
            if count >= limit {
                break;
            }
            if cancel.is_cancelled() {
                return Err(error::aborted());
            }
            let bytes = match self.fs.read(file).await {
                Ok(b) => b,
                Err(_) => continue,
            };
            let sink = GrepSink {
                rel: rel.clone(),
                out: &mut out,
                count: &mut count,
                limit,
                any_line_truncated: &mut any_line_truncated,
            };
            let _ = searcher.search_slice(&matcher, &bytes, sink);
        }

        if out.is_empty() {
            return Ok(ToolResult {
                content: vec![Content::text("No matches found")],
                details: None,
                terminate: false,
            });
        }

        let joined = out.join("\n");
        let t = truncate_head(&joined, TruncOpts::bytes_only(self.opts.max_bytes));

        let mut text = t.content.clone();
        if count >= limit {
            text.push_str(&format!(
                "\n\n[{limit} match limit reached. Use limit={} to see more.]",
                limit.saturating_mul(2)
            ));
        }
        if t.info.truncated {
            text.push_str(&format!(
                "\n\n[Output truncated at {}.]",
                format_size(self.opts.max_bytes)
            ));
        }
        if any_line_truncated {
            text.push_str(&format!(
                "\n\n[Some lines truncated to {GREP_MAX_LINE_LENGTH} chars.]"
            ));
        }

        let details = crate::details::GrepDetails {
            truncation: Some(t.info),
            match_limit_reached: if count >= limit { Some(limit) } else { None },
            lines_truncated: if any_line_truncated { Some(true) } else { None },
        };

        Ok(ToolResult {
            content: vec![Content::text(text)],
            details: serde_json::to_value(details).ok(),
            terminate: false,
        })
    }
}

impl ToolMeta for GrepTool {
    fn description(&self) -> &str {
        "Search file contents recursively (gitignore-aware), returning filepath:line: match."
    }
    fn prompt_snippet(&self) -> Option<&str> {
        Some("grep: search file contents by regex.")
    }
    fn prompt_guidelines(&self) -> &[&str] {
        &["Use `grep` to find code by content; pass `glob` to narrow files and `context` for surrounding lines."]
    }
}
