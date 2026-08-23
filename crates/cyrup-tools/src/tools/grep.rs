//! `grep` — in-process ripgrep-parity search (R-03-029…031, arch-03 §6.6). [CYRUP-DELTA]: uses the
//! `ignore`/`grep` crates instead of an external `rg` binary; output format, gitignore semantics,
//! and truncation preserve Pi's observable behavior.

use crate::config::GrepOpts;
use crate::ops::{FsOps, WalkOpts};
use crate::tools::globmatch::{RgGlob, to_posix};
use crate::truncate::{GREP_MAX_LINE_LENGTH, TruncOpts, format_size, truncate_head, truncate_line};
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
        Self {
            fs,
            cwd,
            opts,
            params,
        }
    }

    /// Search ONE candidate and append its formatted rows to `out`, advancing the GLOBAL match
    /// `count`. Extracted from `execute` so the search can be fused into the walk (see the loop
    /// there) instead of running as a second pass over a fully-drained, sorted file list.
    ///
    /// Pi gathers raw matches first (no context in the searcher), then formats each match as an
    /// independent re-read block; mirror that so overlapping windows duplicate shared context
    /// lines exactly as Pi does (`formatBlock`, grep.ts:255-273).
    #[allow(clippy::too_many_arguments)]
    async fn search_one(
        &self,
        file: &std::path::Path,
        rel: &str,
        matcher: &grep_regex::RegexMatcher,
        context: usize,
        limit: usize,
        count: &mut usize,
        out: &mut Vec<String>,
        any_line_truncated: &mut bool,
    ) -> Result<(), ToolError> {
        // Pi never materializes the candidate in the agent process: the search runs in a separate
        // ripgrep child (`spawn(rgPath, args, …)`, grep.ts:226) with rg's own bounded read buffer,
        // so file size is decoupled from the agent's heap. cyrup's search is in-process (the
        // declared `ignore`/`grep-searcher` delta, module doc above), so the same property comes
        // from [`FsOps::read_stream`] + `search_reader`. A full `FsOps::read` here allocated the
        // whole file BEFORE binary detection could reject it — one multi-GB log, database dump or
        // vendored tarball in the tree was an RSS spike or an OOM kill of the session, on a file
        // that need not match at all. A read failure is skipped, exactly as before: rg simply
        // emits no match events for a file it cannot open.
        let Ok(reader) = self.fs.read_stream(file).await else {
            return Ok(());
        };

        // Binary detection. Pi spawns real ripgrep with no `--text`/`-a` (grep.ts:220-224), so
        // ripgrep's default applies: files reached by traversal are searched with
        // `BinaryDetection::quit(b'\x00')` — a NUL ends that file as if EOF, so a binary file
        // contributes no `--json` match lines. `grep-searcher`'s own default is
        // `BinaryDetection::None` ("Data reported by the searcher may contain arbitrary bytes"),
        // which would dump raw bytes of PNG/wasm/font/sqlite hits into the model-facing result.
        //
        // [CYRUP-DELTA] ripgrep uses `convert(b'\x00')` instead of `quit` for a path named
        // EXPLICITLY on the command line (Pi's `path` argument pointing at a single file). cyrup
        // keeps `quit` there too: `convert` renumbers lines at every NUL, while the output blocks
        // below are cut from a separate raw re-read of the file that splits on `\n` only, so the
        // two numberings would disagree and emit the wrong lines.
        //
        // The searcher is built per file rather than hoisted because `search_reader` is a
        // BLOCKING API driven from `spawn_blocking` (see [`FsOps::read_stream`]), so it and the
        // reader must be owned by the blocking task.
        let matcher_owned = matcher.clone();
        // `MatchSink` counts against the REMAINING budget; the caller's global `count` is advanced
        // by however many this file contributed. Pi's cap is global too — its line handler ignores
        // every event once `matchCount >= effectiveLimit` (grep.ts:278) — so a file can only ever
        // fill the gap.
        let remaining = limit.saturating_sub(*count);
        let matches: Vec<(u64, Vec<u8>)> = tokio::task::spawn_blocking(move || {
            let mut searcher: Searcher = SearcherBuilder::new()
                .line_number(true)
                .binary_detection(BinaryDetection::quit(b'\x00'))
                .build();
            let mut matches: Vec<(u64, Vec<u8>)> = Vec::new();
            let mut local = 0usize;
            {
                let sink = MatchSink {
                    matches: &mut matches,
                    count: &mut local,
                    limit: remaining,
                };
                let _ = searcher.search_reader(&matcher_owned, reader, sink);
            }
            matches
        })
        .await
        .map_err(|e| error::invalid(format!("grep: {e}")))?;

        if matches.is_empty() {
            return Ok(());
        }
        *count += matches.len();

        // Which matches take Pi's `formatBlock` path (grep.ts:333-335) rather than the direct
        // `match.lineText` path (grep.ts:323-331)? Pi's condition is
        // `contextValue === 0 && match.lineText !== undefined`, and `lineText` is
        // `event.data.lines.text` — ABSENT whenever ripgrep could not encode the line as UTF-8
        // (it serialises `lines.bytes` instead). So: context>0, or a non-UTF-8 matched line.
        let takes_block = |raw: &[u8]| context > 0 || std::str::from_utf8(raw).is_err();
        // `formatBlock` reads through `getFileLines`, a **second, independent** read of the file
        // (grep.ts:206-218) — ripgrep's own read was a different process against the real
        // filesystem — cached for the rest of the invocation by `fileCache`. Do it at most once
        // per file, and ONLY if some match actually needs it, so that at context==0 the file is
        // never re-read (Pi does not) yet the read-your-latest-writes semantics and the failure
        // path below stay reachable exactly where Pi has them. Pi's own `fileCache` is moot here:
        // each candidate is now visited exactly once.
        //
        // `None` is Pi's `catch { lines = [] }` (grep.ts:212-214). A successfully-read EMPTY
        // file is NOT that case — `"".split("\n")` is `[""]`, length 1. This one IS a whole-file
        // read on both sides, and pi pays it too — but only for a file that ACTUALLY MATCHED and
        // only on the `contextValue > 0` / non-UTF-8 path.
        let src_lines: Option<Vec<String>> = if matches.iter().any(|(_, r)| takes_block(r)) {
            match self.fs.read(file).await {
                // Pi `getFileLines` folds `\r\n`→`\n` AND lone `\r`→`\n` BEFORE splitting
                // (grep.ts:211). The matcher numbered lines on raw `\n`, so a file using
                // lone-`\r` separators yields context blocks that key off these folded segments
                // — matching Pi even where that diverges from the matcher's numbering.
                Ok(b) => {
                    let content = String::from_utf8_lossy(&b);
                    let folded = content.replace("\r\n", "\n").replace('\r', "\n");
                    Some(folded.split('\n').map(str::to_owned).collect())
                }
                Err(_) => None,
            }
        } else {
            None
        };
        for (ln, raw) in &matches {
            let l = *ln as usize;
            if !takes_block(raw) {
                // Pi grep.ts:325-331: format straight from the captured line text —
                // `\r\n`→`\n`, then DROP every remaining `\r` (not fold it to `\n`, which is
                // what `getFileLines` does), then strip ONE trailing `\n`.
                let stripped = String::from_utf8_lossy(raw)
                    .replace("\r\n", "\n")
                    .replace('\r', "");
                let text = stripped.strip_suffix('\n').unwrap_or(&stripped);
                let (capped, tr) = truncate_line(text, GREP_MAX_LINE_LENGTH);
                if tr {
                    *any_line_truncated = true;
                }
                out.push(format!("{rel}:{l}: {capped}"));
                continue;
            }
            // Pi `formatBlock`: an unreadable file collapses the whole block to ONE marker row
            // per match (grep.ts:258). The rows still count as output, so they participate in
            // byte truncation like any other line.
            let Some(src_lines) = src_lines.as_ref() else {
                out.push(format!("{rel}:{l}: (unable to read file)"));
                continue;
            };
            // Pi: `start = max(1, n - context)`, `end = min(lines.length, n + context)` when
            // context > 0, else just the single match line (grep.ts:260-261).
            let (start, end) = if context > 0 {
                (
                    l.saturating_sub(context).max(1),
                    (l + context).min(src_lines.len()),
                )
            } else {
                (l, l)
            };
            for current in start..=end {
                let raw = src_lines
                    .get(current.saturating_sub(1))
                    .map_or("", String::as_str);
                // Pi's per-line `replace(/\r/g,"")` (grep.ts:264).
                let text = raw.replace('\r', "");
                let (capped, tr) = truncate_line(&text, GREP_MAX_LINE_LENGTH);
                if tr {
                    *any_line_truncated = true;
                }
                if current == l {
                    out.push(format!("{rel}:{current}: {capped}"));
                } else {
                    out.push(format!("{rel}-{current}- {capped}"));
                }
            }
        }
        Ok(())
    }
}

/// Collects the 1-based line number AND the raw bytes of every match in a file, capping the GLOBAL
/// match count at `limit`. Pi counts each rg `match` event (one per matching line) and stops the
/// child once `matchCount >= effectiveLimit` (grep.ts:280-292).
///
/// The bytes are Pi's `event.data.lines.text` (grep.ts:307, kept on the match record at :310): at
/// `context == 0` Pi formats the row straight from that captured text and never re-reads the file
/// (grep.ts:318-326). Only the context>0 path — and the non-UTF-8 fallback, where ripgrep emits
/// `lines.bytes` instead of `lines.text` so `match.lineText` is `undefined` — goes through
/// `formatBlock` → `getFileLines` (grep.ts:250-268), which re-reads and formats an INDEPENDENT
/// block per match, so overlapping context windows DUPLICATE shared lines rather than merging.
struct MatchSink<'a> {
    matches: &'a mut Vec<(u64, Vec<u8>)>,
    count: &'a mut usize,
    limit: usize,
}

impl Sink for MatchSink<'_> {
    type Error = std::io::Error;

    fn matched(&mut self, _s: &Searcher, m: &SinkMatch<'_>) -> Result<bool, std::io::Error> {
        // `SinkMatch::bytes` is the matched line INCLUDING its terminator, which is exactly what
        // ripgrep serialises into `data.lines.text` — hence Pi's `.replace(/\n$/,"")` below.
        self.matches
            .push((m.line_number().unwrap_or(0), m.bytes().to_vec()));
        *self.count += 1;
        Ok(*self.count < self.limit)
    }
}

#[async_trait::async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }
    /// TOOL-045 — pi declares `label` explicitly beside `name` on every built-in
    /// `ToolDefinition` and the two are equal for all seven (`grep.ts:129-130` @v0.83.0). See
    /// [`super::ReadTool::label`] for why the trait default was not left to stand in.
    fn label(&self) -> Option<&str> {
        Some("grep")
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
            .map_err(|_| {
                error::not_found(format!("Path not found: {}", error::show(&search_root)))
            })?;

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
        let limit = input
            .limit
            .map_or(self.opts.limit, crate::jsnum::to_count)
            .max(1);

        // Pi hands `glob` to ripgrep verbatim (grep.ts:218), so it parses as ONE gitignore-style
        // override line — anchored when it contains a `/`, basename-matched when it does not. That
        // is the opposite of the `**/`-prefix rule fd needs, which `find` uses
        // (find.ts:243-252 / [`PatternMatcher`]); wiring `grep` through fd's rule un-anchored every
        // path glob, so `glob: "src/**/*.ts"` also matched `vendor/src/a.ts`.
        let glob = match input.glob.as_deref() {
            Some(g) => RgGlob::build(g)?,
            None => None,
        };

        let mut out: Vec<String> = Vec::new();
        let mut count = 0usize;
        let mut any_line_truncated = false;

        if meta.is_file {
            if cancel.is_cancelled() {
                return Err(error::aborted());
            }
            let rel = search_root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| to_posix(&search_root));
            self.search_one(
                &search_root,
                &rel,
                &matcher,
                context,
                limit,
                &mut count,
                &mut out,
                &mut any_line_truncated,
            )
            .await?;
        } else {
            // Pi runs plain `rg --hidden` with NO `--no-require-git` flag (grep.ts:215-219): search
            // dotfiles/dot-dirs, but honor `.gitignore` only *inside* a git repo — ripgrep's default,
            // which is `ignore`'s `require_git:true` (the crate does the in-repo detection internally).
            // Unlike Pi's `find` (fd `--no-require-git` outside a repo, find.ts:226-240), Pi's grep
            // never disables require-git, so outside any repo a stray `.gitignore` is NOT applied.
            let mut walk = self.fs.walk(
                &search_root,
                WalkOpts {
                    include_hidden: true,
                    require_git: true,
                },
            );
            loop {
                // The walk and the search are FUSED, and both stop at the limit. Pi's line handler
                // sets `matchLimitReached` and calls `stopChild(true)` (grep.ts:292-295, defined at
                // `:240-245`) the instant `matchCount >= effectiveLimit`, which kills the rg child
                // — so upstream neither finishes the traversal nor reads another file. Staging the
                // whole walk into a `Vec`, sorting it, and only then searching cost a complete
                // tree walk on EVERY call regardless of `limit`, and took the 100-match window
                // from the alphabetically-first files rather than from the first matches
                // discovered — a systematic `a*`-biased sample with nothing in the output to
                // distinguish it. `grep -n sort grep.ts` is empty at v0.84.1, so the sort is
                // dropped rather than moved.
                if count >= limit {
                    break;
                }
                // The `select!` below only observes a cancel while it is parked on `walk.next()`;
                // one that lands while `search_one` is running is observed here, on the next turn
                // — the same per-candidate granularity the staged loop had.
                if cancel.is_cancelled() {
                    return Err(error::aborted());
                }
                tokio::select! {
                    _ = cancel.cancelled() => return Err(error::aborted()),
                    item = walk.next() => {
                        match item {
                            Some(Ok(w)) if !w.is_dir => {
                                let rel_path = w.path.strip_prefix(&search_root).unwrap_or(&w.path);
                                let rel = to_posix(rel_path);
                                // The glob is matched against the path relative to the OVERRIDE
                                // ROOT, which for ripgrep is its own cwd — Pi spawns `rg` with no
                                // `cwd` option and passes `searchPath` positionally, so a `path`
                                // argument narrows the walk but does NOT re-anchor the glob
                                // (ignore-0.4.33 gitignore.rs:286-315 `strip`). A candidate outside
                                // the root keeps its full path, as `strip` leaves it.
                                let glob_rel = w
                                    .path
                                    .strip_prefix(&self.cwd)
                                    .map_or_else(|_| to_posix(&w.path), to_posix);
                                if let Some(g) = &glob
                                    && !g.keeps_file(&glob_rel) {
                                        continue;
                                    }
                                self.search_one(
                                    &w.path,
                                    &rel,
                                    &matcher,
                                    context,
                                    limit,
                                    &mut count,
                                    &mut out,
                                    &mut any_line_truncated,
                                )
                                .await?;
                            }
                            Some(Ok(_)) => {}
                            Some(Err(e)) => return Err(e),
                            None => break,
                        }
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
            notices.push(format!(
                "{} limit reached",
                format_size(crate::truncate::DEFAULT_MAX_BYTES)
            ));
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
        let details =
            if truncation.is_some() || match_limit_reached.is_some() || lines_truncated.is_some() {
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
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

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

        let fs = Arc::new(FailSecondRead {
            inner: LocalFs,
            reads: AtomicUsize::new(0),
        });
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

        let fs = Arc::new(FailSecondRead {
            inner: LocalFs,
            reads: AtomicUsize::new(0),
        });
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

    async fn grep_text(cwd: &Path, args: serde_json::Value) -> String {
        let grep = GrepTool::new(Arc::new(LocalFs), cwd.to_path_buf(), GrepOpts::default());
        let r = grep
            .execute(
                ToolCallId::from("tc-test"),
                args,
                CancelToken::new(),
                Box::new(|_u: ToolUpdate| {}),
            )
            .await
            .unwrap();
        match r.content.first() {
            Some(Content::Text { text, .. }) => text.clone(),
            _ => String::new(),
        }
    }

    /// `glob` goes to ripgrep verbatim (grep.ts:218), so a pattern containing `/` is ANCHORED at
    /// the override root (ignore-0.4.33 gitignore.rs:513-522 prefixes `**/` only when the pattern
    /// has no `/`). `grep` used to compile the pattern with fd's opposite rule — the one `find`
    /// needs (find.ts:243-252) — which un-anchored it, so `src/**/*.ts` also matched
    /// `vendor/src/*.ts` and those extra hits crowd out real ones against the 100-match cap.
    #[tokio::test]
    async fn path_glob_is_anchored_at_the_override_root() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_path_buf();
        std::fs::create_dir_all(cwd.join("src")).unwrap();
        std::fs::create_dir_all(cwd.join("vendor/src")).unwrap();
        std::fs::write(cwd.join("src/a.ts"), "NEEDLE\n").unwrap();
        std::fs::write(cwd.join("vendor/src/b.ts"), "NEEDLE\n").unwrap();

        let text = grep_text(
            &cwd,
            serde_json::json!({ "pattern": "NEEDLE", "glob": "src/**/*.ts" }),
        )
        .await;
        assert_eq!(text, "src/a.ts:1: NEEDLE");
    }

    /// gitignore.rs:492-498 strips a leading `/` and anchors on it. Passing it through to globset
    /// untouched matched it against a root-relative path that never starts with `/`, so the whole
    /// query silently returned "No matches found".
    #[tokio::test]
    async fn leading_slash_glob_anchors_instead_of_matching_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_path_buf();
        std::fs::create_dir_all(cwd.join("src")).unwrap();
        std::fs::create_dir_all(cwd.join("vendor/src")).unwrap();
        std::fs::write(cwd.join("src/a.ts"), "NEEDLE\n").unwrap();
        std::fs::write(cwd.join("vendor/src/b.ts"), "NEEDLE\n").unwrap();

        let text = grep_text(
            &cwd,
            serde_json::json!({ "pattern": "NEEDLE", "glob": "/src/*.ts" }),
        )
        .await;
        assert_eq!(text, "src/a.ts:1: NEEDLE");
    }

    /// A `path` argument narrows the walk but does NOT re-anchor the glob: Pi spawns `rg` with no
    /// `cwd` option, so ripgrep's override root stays the agent's cwd and the glob is matched
    /// against the cwd-relative path (gitignore.rs:286-315 `strip`). Output paths remain relative
    /// to the search root, as `rg` prints what it was given.
    #[tokio::test]
    async fn glob_is_matched_against_the_cwd_relative_path_not_the_search_root() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_path_buf();
        std::fs::create_dir_all(cwd.join("src")).unwrap();
        std::fs::write(cwd.join("src/a.ts"), "NEEDLE\n").unwrap();

        let text = grep_text(
            &cwd,
            serde_json::json!({ "pattern": "NEEDLE", "path": "src", "glob": "src/*.ts" }),
        )
        .await;
        assert_eq!(text, "a.ts:1: NEEDLE");
    }

    /// A bare pattern (no `/`) is basename-matched at any depth in BOTH rules — the schema's own
    /// first example (`'*.ts'`, grep.rs description) must keep working.
    #[tokio::test]
    async fn bare_glob_still_matches_basenames_at_any_depth() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_path_buf();
        std::fs::create_dir_all(cwd.join("src/deep")).unwrap();
        std::fs::write(cwd.join("src/deep/a.ts"), "NEEDLE\n").unwrap();
        std::fs::write(cwd.join("b.js"), "NEEDLE\n").unwrap();

        let text = grep_text(
            &cwd,
            serde_json::json!({ "pattern": "NEEDLE", "glob": "*.ts" }),
        )
        .await;
        assert_eq!(text, "src/deep/a.ts:1: NEEDLE");
    }
}
