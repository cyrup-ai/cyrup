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
use grep_searcher::{Searcher, SearcherBuilder, Sink, SinkMatch};
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
        // Byte-for-byte Pi's TypeBox emission (grep.ts:24-36): verbatim descriptions,
        // `type:"number"`, no `minimum`, no `additionalProperties`.
        let params = serde_json::json!({
            "type": "object",
            "required": ["pattern"],
            "properties": {
                "pattern": { "type": "string", "description": "Search pattern (regex or literal string)" },
                "path": { "type": "string", "description": "Directory or file to search (default: current directory)" },
                "glob": { "type": "string", "description": "Filter files by glob pattern, e.g. '*.ts' or '**/*.spec.ts'" },
                "ignoreCase": { "type": "boolean", "description": "Case-insensitive search (default: false)" },
                "literal": { "type": "boolean", "description": "Treat pattern as literal string instead of regex (default: false)" },
                "context": { "type": "number", "description": "Number of lines to show before and after each match (default: 0)" },
                "limit": { "type": "number", "description": "Maximum number of matches to return (default: 100)" }
            }
        });
        Self { fs, cwd, opts, params }
    }
}

/// Collects the 1-based line number of every match in a file, capping the GLOBAL match count at
/// `limit`. Pi counts each rg `match` event (one per matching line) and stops the child once
/// `matchCount >= effectiveLimit` (grep.ts:280-292). Context is NOT gathered here — Pi re-reads the
/// file and formats an INDEPENDENT block per match afterwards (grep.ts:250-268,316-331), so
/// overlapping context windows DUPLICATE shared lines (one copy per block) rather than merging.
struct MatchSink<'a> {
    lines: &'a mut Vec<u64>,
    count: &'a mut usize,
    limit: usize,
}

impl Sink for MatchSink<'_> {
    type Error = std::io::Error;

    fn matched(&mut self, _s: &Searcher, m: &SinkMatch<'_>) -> Result<bool, std::io::Error> {
        self.lines.push(m.line_number().unwrap_or(0));
        *self.count += 1;
        Ok(*self.count < self.limit)
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
            // Pi: `Path not found: ${searchPath}` (grep.ts:186).
            .map_err(|_| error::not_found(format!("Path not found: {}", error::show(&search_root))))?;

        let matcher = RegexMatcherBuilder::new()
            .case_insensitive(input.ignore_case.unwrap_or(false))
            .fixed_strings(input.literal.unwrap_or(false))
            .build(&input.pattern)
            .map_err(|e| error::invalid(format!("grep: invalid pattern: {e}")))?;

        let context = input.context.unwrap_or(0);
        // Pi: `effectiveLimit = Math.max(1, limit ?? DEFAULT_LIMIT)` (grep.ts:189). The JSON-schema
        // `minimum:1` is advisory only, so an explicit `limit: 0` must still yield up to one match
        // rather than short-circuiting to "No matches found".
        let limit = input.limit.unwrap_or(self.opts.limit).max(1);

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
            // Pi runs plain `rg --hidden` with NO `--no-require-git` flag (grep.ts:215-219): search
            // dotfiles/dot-dirs, but honor `.gitignore` only *inside* a git repo — ripgrep's default,
            // which is `ignore`'s `require_git:true` (the crate does the in-repo detection internally).
            // Unlike Pi's `find` (fd `--no-require-git` outside a repo, find.ts:226-240), Pi's grep
            // never disables require-git, so outside any repo a stray `.gitignore` is NOT applied.
            let mut walk =
                self.fs.walk(&search_root, WalkOpts { include_hidden: true, require_git: true });
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
                                if let Some(g) = &glob
                                    && !g.is_match(&rel, &basename) {
                                        continue;
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

        // Pi gathers raw matches first (no context in the searcher), then formats each match as an
        // independent re-read block; mirror that so overlapping windows duplicate shared context
        // lines exactly as Pi does (grep.ts:250-268).
        let mut searcher: Searcher = SearcherBuilder::new().line_number(true).build();

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
            let mut match_lines: Vec<u64> = Vec::new();
            {
                let sink = MatchSink { lines: &mut match_lines, count: &mut count, limit };
                let _ = searcher.search_slice(&matcher, &bytes, sink);
            }
            if match_lines.is_empty() {
                continue;
            }
            // Pi: `content.replace(/\r\n/g,"\n").replace(/\r/g,"\n").split("\n")` then a per-line
            // `replace(/\r/g,"")` (grep.ts:206,259). Splitting the same bytes the searcher numbered
            // on `\n` keeps line numbers aligned; CR removal happens per output line below.
            let content = String::from_utf8_lossy(&bytes);
            // Pi `getFileLines` folds `\r\n`→`\n` AND lone `\r`→`\n` BEFORE splitting
            // (grep.ts:206). The matcher numbered lines on raw `\n`, so a file using lone-`\r`
            // separators yields context blocks that key off these folded segments — matching Pi
            // even where that diverges from the matcher's `\n`-based numbering.
            let folded = content.replace("\r\n", "\n").replace('\r', "\n");
            let src_lines: Vec<&str> = folded.split('\n').collect();
            for &ln in &match_lines {
                let l = ln as usize;
                // Pi: `start = max(1, n - context)`, `end = min(lines.length, n + context)` when
                // context > 0, else just the single match line (grep.ts:255-256).
                let (start, end) = if context > 0 {
                    (l.saturating_sub(context).max(1), (l + context).min(src_lines.len()))
                } else {
                    (l, l)
                };
                for current in start..=end {
                    let raw = src_lines.get(current.saturating_sub(1)).copied().unwrap_or("");
                    let text = raw.replace('\r', "");
                    let (capped, tr) = truncate_line(&text, GREP_MAX_LINE_LENGTH);
                    if tr {
                        any_line_truncated = true;
                    }
                    if current == l {
                        out.push(format!("{rel}:{current}: {capped}"));
                    } else {
                        out.push(format!("{rel}-{current}- {capped}"));
                    }
                }
            }
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

        // Pi joins every notice into ONE bracket (grep.ts:338-356), e.g.
        // `[100 matches limit reached. Use limit=200 for more, or refine pattern. 50.0KB limit
        //   reached. Some lines truncated to 500 chars. Use read tool to see full lines]`.
        let mut text = t.content.clone();
        let mut notices: Vec<String> = Vec::new();
        if count >= limit {
            notices.push(format!(
                "{limit} matches limit reached. Use limit={} for more, or refine pattern",
                limit.saturating_mul(2)
            ));
        }
        if t.info.truncated {
            // Pi hardcodes `formatSize(DEFAULT_MAX_BYTES)` (grep.ts:347).
            notices.push(format!("{} limit reached", format_size(crate::truncate::DEFAULT_MAX_BYTES)));
        }
        if any_line_truncated {
            notices.push(format!(
                "Some lines truncated to {GREP_MAX_LINE_LENGTH} chars. Use read tool to see full lines"
            ));
        }
        if !notices.is_empty() {
            text.push_str(&format!("\n\n[{}]", notices.join(". ")));
        }

        // Pi only adds `truncation` to details when the byte cap actually fired, and emits
        // `details: undefined` when no key is set (grep.ts:337-360). Mirror that exactly.
        let match_limit_reached = if count >= limit { Some(limit) } else { None };
        let lines_truncated = if any_line_truncated { Some(true) } else { None };
        let truncation = if t.info.truncated { Some(t.info) } else { None };
        let details = if truncation.is_some()
            || match_limit_reached.is_some()
            || lines_truncated.is_some()
        {
            serde_json::to_value(crate::details::GrepDetails {
                truncation,
                match_limit_reached,
                lines_truncated,
            })
            .ok()
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

impl ToolMeta for GrepTool {
    // Verbatim from Pi (grep.ts:131-132). DEFAULT_LIMIT=100, DEFAULT_MAX_BYTES/1024=50,
    // GREP_MAX_LINE_LENGTH=500. Pi defines no promptGuidelines for grep.
    fn description(&self) -> &str {
        "Search file contents for a pattern. Returns matching lines with file paths and line \
         numbers. Respects .gitignore. Output is truncated to 100 matches or 50KB (whichever is \
         hit first). Long lines are truncated to 500 chars."
    }
    fn prompt_snippet(&self) -> Option<&str> {
        Some("Search file contents for patterns (respects .gitignore)")
    }
}
