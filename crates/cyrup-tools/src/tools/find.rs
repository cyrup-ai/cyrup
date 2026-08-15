//! `find` — in-process fd-parity glob search (R-03-032…034, arch-03 §6.7). [CYRUP-DELTA]: uses
//! `ignore::WalkBuilder` + `globset` instead of an external `fd` binary.

use crate::config::FindOpts;
use crate::details::FindDetails;
use crate::ops::{FsOps, WalkOpts};
use crate::tools::globmatch::{to_posix, PatternMatcher};
use crate::truncate::{format_size, truncate_head, TruncOpts};
use crate::{error, path};
use cyrup_core::{CancelToken, Content, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink};
use futures::StreamExt;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct FindInput {
    pattern: String,
    path: Option<String>,
    // Pi's TypeBox `Type.Number` (find.ts:25) carries no `integer` and no `minimum`, and Pi never
    // validates tool arguments at runtime, so `limit: 1000.0` and `limit: -1` are inputs Pi
    // accepts. Modeling this as `usize` rejected the whole call at deserialization. See
    // [`crate::jsnum`].
    limit: Option<f64>,
}

pub struct FindTool {
    fs: Arc<dyn FsOps>,
    cwd: PathBuf,
    opts: FindOpts,
    params: serde_json::Value,
}

impl FindTool {
    pub fn new(fs: Arc<dyn FsOps>, cwd: PathBuf, opts: FindOpts) -> Self {
        // Byte-for-byte Pi's TypeBox emission (find.ts:20-26): verbatim descriptions,
        // `type:"number"`, no `minimum`, no `additionalProperties`.
        let params = serde_json::json!({
            "type": "object",
            "required": ["pattern"],
            "properties": {
                "pattern": { "type": "string", "description": "Glob pattern to match files, e.g. '*.ts', '**/*.json', or 'src/**/*.spec.ts'" },
                "path": { "type": "string", "description": "Directory to search in (default: current directory)" },
                "limit": { "type": "number", "description": "Maximum number of results (default: 1000)" }
            }
        });
        Self { fs, cwd, opts, params }
    }

    /// Pi's git-repo probe (find.ts:230-239): walk from `search_path` up through its parents and
    /// report whether any ancestor (inclusive) contains a `.git` entry. Mirrors Pi's
    /// `pathExists(path.join(current, ".git"))` loop that stops at the filesystem root. The `.git`
    /// probe goes through the [`FsOps`] seam so remote backends resolve against the correct
    /// filesystem; a `.git` file (worktrees/submodules) or directory both count.
    async fn inside_git_repo(&self, search_path: &std::path::Path) -> bool {
        let mut current = search_path;
        loop {
            if self.fs.metadata(&current.join(".git")).await.is_ok() {
                return true;
            }
            match current.parent() {
                // Pi breaks when `dirname(current) === current` (the root fixpoint).
                Some(parent) if parent != current => current = parent,
                _ => return false,
            }
        }
    }
}

#[async_trait::async_trait]
impl Tool for FindTool {
    fn name(&self) -> &str {
        "find"
    }
    /// TOOL-045 — pi declares `label` explicitly beside `name` on every built-in
    /// `ToolDefinition` and the two are equal for all seven (`find.ts:115-116` @v0.83.0). See
    /// [`super::ReadTool::label`] for why the trait default was not left to stand in.
    fn label(&self) -> Option<&str> {
        Some("find")
    }
    fn parameters(&self) -> &serde_json::Value {
        &self.params
    }

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

    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: serde_json::Value,
        cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        // Pi's FIRST statement inside the executor, before `resolveToCwd`, before `ops.exists`,
        // before anything: `if (signal?.aborted) { reject(new Error("Operation aborted")); return; }`
        // (find.ts:142-145). cyrup observed the token for the first time only at the walk loop's
        // `select!`, so an already-cancelled `find` still paid `fs.metadata(search_root)` AND the
        // whole `inside_git_repo` ancestor walk — one `metadata` per parent up to the filesystem
        // root — before it could report the abort. (find.ts has no parameter validation at all, so
        // "abort first, parse second" is also pi's order.)
        if cancel.is_cancelled() {
            return Err(error::aborted());
        }

        let input: FindInput =
            serde_json::from_value(params).map_err(|e| error::invalid(format!("find: {e}")))?;

        let search_root = path::resolve_to_cwd(input.path.as_deref().unwrap_or("."), &self.cwd);
        self.fs
            .metadata(&search_root)
            .await
            // Pi: `Path not found: ${searchPath}` (find.ts:158).
            .map_err(|_| error::not_found(format!("Path not found: {}", error::show(&search_root))))?;

        let matcher = PatternMatcher::build(&input.pattern)?;
        // Pi: `const effectiveLimit = limit ?? DEFAULT_LIMIT` (find.ts:151), handed straight to
        // `fd --max-results ${effectiveLimit}` (find.ts:241). A non-positive count yields no rows
        // from that cap, so folding negatives to 0 reproduces the observable result — an empty
        // match set — without the deserialization failure. `??` is null/undefined-only, so a JSON
        // `null` takes the default.
        let limit = input.limit.map_or(self.opts.limit, crate::jsnum::to_count);

        // Pi (find.ts:226-240, issue #5960): fd normally ignores `.gitignore` OUTSIDE a git repo, so
        // it passes `--no-require-git` there. INSIDE a repo it uses fd's default git-aware behavior
        // so parent `.gitignore` rules stop at nested repo boundaries. Walk parents of the search
        // path looking for a `.git` entry to decide which mode to use.
        let inside_git_repo = self.inside_git_repo(&search_root).await;

        let mut results: Vec<String> = Vec::new();
        // Pi runs `fd --hidden` (find.ts:224): match dotfiles/dot-dirs while still honoring
        // `.gitignore` (arch-03:430). So include hidden files in the walk.
        let mut walk = self
            .fs
            .walk(&search_root, WalkOpts { include_hidden: true, require_git: inside_git_repo });
        loop {
            // `--max-results` (find.ts:252) makes **fd itself** stop traversing once it has emitted
            // N paths; pi never sees the rest of the tree. Draining the whole walk and then
            // `sort()`+`truncate()` cost the full-tree walk on every call regardless of `limit`,
            // AND returned a different result SET — the alphabetically-first N rather than the
            // first N discovered. Both are fixed by bounding the walk here. `grep -n sort find.ts`
            // is empty at v0.84.1: pi relativizes only the lines it received (find.ts:321-326) and
            // never reorders them, so the sort is dropped rather than moved. Note this bounds the
            // stream, not just the vector: dropping out of the loop closes the receiver and
            // `LocalFs::walk`'s producer task breaks on the send error (ops/local.rs).
            if results.len() >= limit {
                break;
            }
            // Pi re-tests `signal?.aborted` FIRST on every data path (find.ts:174, :182, :226,
            // :299, :355), so data can never win a race against an already-fired abort. The
            // `select!` below only observes a cancel while it is parked on `walk.next()`; one that
            // lands while the previous entry was being matched is observed here, on the next turn.
            // The sibling `grep.rs` already carries this guard.
            if cancel.is_cancelled() {
                return Err(error::aborted());
            }
            tokio::select! {
                // `biased;` — without it `select!` polls in RANDOM order, so with the token
                // already cancelled AND an entry already buffered the walk arm won half the time
                // and the tool kept consuming directory entries after Esc: bounded in expectation,
                // unbounded in the worst case. Pi's abort is deterministic on both edges (the
                // `{once:true}` listener at find.ts:158-160 rejects the instant the signal fires),
                // so the cancel arm must be polled first here.
                biased;
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
                            // fd `--full-path` "matches against the absolute candidate path"
                            // (find.ts:254-256, pi's own in-source note), and find.ts:267
                            // `args.push("--", effectivePattern, searchPath)` hands fd the
                            // ABSOLUTE search path as its root, so every candidate fd tests is
                            // absolute. Matching the search-root-RELATIVE path instead made the
                            // `pattern.starts_with('/')` arm in `PatternMatcher::build`
                            // (globmatch.rs) dead — a relative posix path can never begin with `/`
                            // — so a leading-slash pattern like `/src/**/*.ts` silently returned
                            // the empty set. Relativization stays for OUTPUT only (find.ts:321-326).
                            let abs_posix = to_posix(&w.path);
                            if matcher.is_match(&abs_posix, &basename) {
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

        // Pi's `results.length === 0` check (find.ts:311) runs on rows that `fd` has ALREADY
        // capped with `--max-results` (find.ts:252), so the cap is applied before the empty test —
        // which the bounded walk above preserves. Only distinguishable with `limit` folded to 0 (a
        // non-positive argument); for every positive limit the two orders agree.
        if results.is_empty() {
            return Ok(ToolResult {
                content: vec![Content::text("No files found matching pattern")],
                details: None,
                terminate: false,
                ..Default::default()
            });
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
            // Pi hardcodes `formatSize(DEFAULT_MAX_BYTES)` (find.ts:335).
            notices.push(format!("{} limit reached", format_size(crate::truncate::DEFAULT_MAX_BYTES)));
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
            ..Default::default()
        })
    }
}
