//! `grep` — in-process ripgrep-parity search (R-03-029…031, arch-03 §6.6). [CYRUP-DELTA]: uses the
//! `ignore`/`grep` crates instead of an external `rg` binary; output format, gitignore semantics,
//! and truncation preserve Pi's observable behavior.

use crate::config::GrepOpts;
use crate::ops::{FsOps, WalkOpts};
use crate::tools::globmatch::{to_posix, PatternMatcher};
use crate::truncate::{format_size, truncate_head, truncate_line, GREP_MAX_LINE_LENGTH, TruncOpts};
use crate::{error, path};
use cyrup_core::{CancelToken, Content, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink};
use futures::StreamExt;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkMatch};
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
    // Pi's TypeBox `Type.Number` (grep.ts:31-34) carries no `integer` and no `minimum`, and Pi
    // never validates tool arguments at runtime — it clamps them at the point of use instead
    // (grep.ts:188-189). Modeling these as `usize` rejected `context: 0.0` / `limit: -1` at
    // deserialization, where Pi returns a normal result. See [`crate::jsnum`].
    context: Option<f64>,
    limit: Option<f64>,
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

        // Pi: `const contextValue = context && context > 0 ? context : 0` (grep.ts:188) — a
        // negative, zero or NaN `context` all collapse to 0 instead of failing. `to_count` folds
        // the fraction the way `lineNumber ± contextValue` indexing would (grep.ts:254-255).
        let context = input.context.map_or(0, crate::jsnum::to_count);
        // Pi: `effectiveLimit = Math.max(1, limit ?? DEFAULT_LIMIT)` (grep.ts:189). The JSON-schema
        // `minimum:1` is advisory only, so an explicit `limit: 0` — or a negative one, which the
        // same `Math.max` absorbs — must still yield up to one match rather than short-circuiting
        // to "No matches found". `??` is null/undefined-only, so a JSON `null` also takes the
        // default.
        let limit = input.limit.map_or(self.opts.limit, crate::jsnum::to_count).max(1);

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
        //
        // Binary detection. Pi spawns real ripgrep with no `--text`/`-a` (grep.ts:215-220), so
        // ripgrep's default applies: files reached by traversal are searched with
        // `BinaryDetection::quit(b'\x00')` — a NUL ends that file as if EOF, so a binary file
        // contributes no `--json` match lines. `grep-searcher`'s own default is
        // `BinaryDetection::None` ("Data reported by the searcher may contain arbitrary bytes"),
        // which would dump raw bytes of PNG/wasm/font/sqlite hits into the model-facing result.
        // On a slice search `quit` scans the first 8 KiB plus every matched line (glue.rs:118-121,
        // core.rs:215-237) — the same conservative rule ripgrep uses for memory-mapped files.
        //
        // [CYRUP-DELTA] ripgrep uses `convert(b'\x00')` instead of `quit` for a path named
        // EXPLICITLY on the command line (Pi's `path` argument pointing at a single file). cyrup
        // keeps `quit` there too: `convert` renumbers lines at every NUL, while the output blocks
        // below are cut from a separate raw re-read of the file that splits on `\n` only, so the
        // two numberings would disagree and emit the wrong lines.
        let mut searcher: Searcher = SearcherBuilder::new()
            .line_number(true)
            .binary_detection(BinaryDetection::quit(b'\x00'))
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
            let mut match_lines: Vec<u64> = Vec::new();
            {
                let sink = MatchSink { lines: &mut match_lines, count: &mut count, limit };
                let _ = searcher.search_slice(&matcher, &bytes, sink);
            }
            if match_lines.is_empty() {
                continue;
            }
            // Pi formats a context block from a **second, independent** read: `formatBlock` calls
            // `getFileLines`, which pulls the file through the injectable `ops.readFile` and caches
            // it per invocation (grep.ts:200-213,250-268) — ripgrep's own read is a different
            // process against the real filesystem. Only the context>0 path re-reads; at context==0
            // Pi formats straight from ripgrep's captured `data.lines.text` and never touches the
            // file again (grep.ts:316-326). Mirror both halves: re-read only when context>0, reuse
            // the search bytes otherwise. That keeps Pi's observable read-your-latest-writes
            // semantics (a file rewritten between match and format renders with the NEW content,
            // clamped by the new `lines.length`) and, crucially, keeps Pi's failure path reachable.
            let format_bytes = if context > 0 { self.fs.read(file).await.ok() } else { Some(bytes) };
            // Pi: `catch { lines = [] }` in `getFileLines` (grep.ts:207-209), and `formatBlock`
            // turns an empty `lines` into ONE marker row per match, in place of the whole context
            // block: `` if (!lines.length) return [`${relativePath}:${lineNumber}: (unable to read
            // file)`] `` (grep.ts:253). A successfully-read empty file is NOT this case — `"".
            // split("\n")` is `[""]`, length 1 — so only a read failure emits the marker. The rows
            // still count as output, so they participate in byte truncation like any other line.
            let Some(bytes) = format_bytes else {
                for &ln in &match_lines {
                    out.push(format!("{rel}:{ln}: (unable to read file)"));
                }
                continue;
            };
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
                ..Default::default()
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
            ..Default::default()
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::GrepTool;
    use crate::config::GrepOpts;
    use crate::ops::local::LocalFs;
    use crate::ops::{Access, DirEntry, FsOps, Meta, WalkItem, WalkOpts};
    use cyrup_core::{CancelToken, Content, EventStream, Tool, ToolCallId, ToolError, ToolUpdate};
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// `LocalFs` that serves the first `read()` of any path normally and fails every later one.
    ///
    /// This models Pi's two-reader shape exactly: ripgrep matches the file from its own process
    /// while the format pass re-reads it through the injectable `ops.readFile` (grep.ts:200-213),
    /// so the format read can fail on a file that demonstrably matched — the file is unlinked, its
    /// mode is dropped to `0000`, or a custom `readFile` backend goes away between the two reads.
    struct FailSecondRead {
        inner: LocalFs,
        reads: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl FsOps for FailSecondRead {
        async fn read(&self, path: &Path) -> Result<Vec<u8>, ToolError> {
            if self.reads.fetch_add(1, Ordering::SeqCst) == 0 {
                self.inner.read(path).await
            } else {
                Err(ToolError::new("EACCES"))
            }
        }
        async fn write_in_place(&self, path: &Path, bytes: &[u8]) -> Result<(), ToolError> {
            self.inner.write_in_place(path, bytes).await
        }
        async fn access(&self, path: &Path, mode: Access) -> Result<(), ToolError> {
            self.inner.access(path, mode).await
        }
        async fn metadata(&self, path: &Path) -> Result<Meta, ToolError> {
            self.inner.metadata(path).await
        }
        async fn read_dir(&self, path: &Path) -> Result<Vec<DirEntry>, ToolError> {
            self.inner.read_dir(path).await
        }
        fn walk(&self, root: &Path, opts: WalkOpts) -> EventStream<Result<WalkItem, ToolError>> {
            self.inner.walk(root, opts)
        }
    }

    /// Pi `formatBlock` (grep.ts:250-253): when `getFileLines` cannot produce lines it emits ONE
    /// `<path>:<line>: (unable to read file)` row per match in place of the context block, rather
    /// than dropping the file from the output. cyrup previously `continue`d on a read failure, so
    /// the file vanished silently and the marker string appeared nowhere in the workspace.
    #[tokio::test]
    async fn context_block_emits_unable_to_read_marker_when_reread_fails() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_path_buf();
        std::fs::write(cwd.join("a.txt"), "one\nNEEDLE\nthree\n").unwrap();

        let fs = Arc::new(FailSecondRead { inner: LocalFs, reads: AtomicUsize::new(0) });
        let grep = GrepTool::new(fs, cwd.clone(), GrepOpts::default());
        let r = grep
            .execute(
                ToolCallId::from("tc-test"),
                serde_json::json!({ "pattern": "NEEDLE", "context": 1 }),
                CancelToken::new(),
                Box::new(|_u: ToolUpdate| {}),
            )
            .await
            .unwrap();

        let text = match r.content.first() {
            Some(Content::Text { text, .. }) => text.clone(),
            _ => String::new(),
        };
        // Exactly Pi's row: match separator `:`, then the parenthesized marker. No context rows
        // (`a.txt-1-` / `a.txt-3-`) accompany it, because the block was replaced, not augmented.
        assert_eq!(text, "a.txt:2: (unable to read file)");
    }

    /// The context==0 path must NOT re-read: Pi formats it from ripgrep's own captured
    /// `data.lines.text` and never calls `getFileLines` (grep.ts:316-326), so a backend that would
    /// fail a second read still renders the real line.
    #[tokio::test]
    async fn zero_context_never_rereads_and_so_never_marks() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_path_buf();
        std::fs::write(cwd.join("a.txt"), "one\nNEEDLE\nthree\n").unwrap();

        let fs = Arc::new(FailSecondRead { inner: LocalFs, reads: AtomicUsize::new(0) });
        let grep = GrepTool::new(fs, cwd.clone(), GrepOpts::default());
        let r = grep
            .execute(
                ToolCallId::from("tc-test"),
                serde_json::json!({ "pattern": "NEEDLE" }),
                CancelToken::new(),
                Box::new(|_u: ToolUpdate| {}),
            )
            .await
            .unwrap();

        let text = match r.content.first() {
            Some(Content::Text { text, .. }) => text.clone(),
            _ => String::new(),
        };
        assert_eq!(text, "a.txt:2: NEEDLE");
    }
}
