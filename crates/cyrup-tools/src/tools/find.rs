//! `find` — in-process fd-parity glob search (R-03-032…034, arch-03 §6.7). [CYRUP-DELTA]: uses
//! `ignore::WalkBuilder` + `globset` instead of an external `fd` binary.

use crate::config::FindOpts;
use crate::details::FindDetails;
use crate::ops::{FsOps, WalkFlavor, WalkOpts};
use crate::tools::globmatch::{PatternMatcher, to_posix};
use crate::truncate::{TruncOpts, format_size, truncate_head};
use crate::{error, path};
use cyrup_core::{CancelToken, Content, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink};
use futures::StreamExt;
use std::path::{Path, PathBuf};
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
        Self {
            fs,
            cwd,
            opts,
            params,
        }
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
        // Pi's fd branch has NO pre-check: it hands the absolute search path to fd as fd's root
        // (find.ts:267) and lets fd validate it. fd's gate is ONE predicate — `is_existing_directory`
        // = `path.is_dir() && …` (fd/src/filesystem.rs:38-42) — so a MISSING path and a path that
        // exists but is not a directory take the same branch and print the same line via
        // `print_error` (fd/src/error.rs), which prefixes `[fd error]: `. With the only root
        // filtered out, `search_paths()` returns empty and fd bails through the same prefix
        // (main.rs:84-86, :68-71), exiting 1 with empty stdout. pi rejects with `stderr.trim()`
        // (find.ts:304-309), i.e. both lines. So: one gate here, one two-line message, and the
        // `Path not found:` literal — which is pi's `customOps.glob` branch (find.ts:171), NOT the
        // fd branch this tool implements — does not belong on this path.
        // `FsOps::metadata` follows symlinks (ops/local/fs.rs:170-183), matching `Path::is_dir`, so
        // a symlink to a directory is still a valid search root.
        if !self
            .fs
            .metadata(&search_root)
            .await
            .is_ok_and(|meta| meta.is_dir)
        {
            return Err(error::invalid(format!(
                "[fd error]: Search path '{}' is not a directory.\n\
                 [fd error]: No valid search paths given.",
                error::show(&search_root)
            )));
        }

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
        let mut walk = self.fs.walk(
            &search_root,
            WalkOpts {
                include_hidden: true,
                require_git: inside_git_repo,
                // Pi's `find` IS fd (find.ts:225 `ensureTool("fd")`) invoked with no
                // `--no-ignore`/`--no-global-ignore-file` (find.ts:235-267), so fd's full default
                // ignore set is in force: `.fdignore` files plus `<config>/fd/ignore`.
                flavor: WalkFlavor::Fd,
                // `find` is fd, not ripgrep: it does not read `$RIPGREP_CONFIG_PATH`,
                // so every walk knob that flag file controls stays at its default here.
                ..WalkOpts::default()
            },
        );
        loop {
            // `--max-results` (find.ts:252) makes **fd itself** stop traversing once it has emitted
            // N paths; pi never sees the rest of the tree. Draining the whole walk and then
            // `sort()`+`truncate()` cost the full-tree walk on every call regardless of `limit`,
            // AND returned a different result SET — the alphabetically-first N rather than the
            // first N discovered. Both are fixed by bounding the walk here.
            //
            // `grep -n sort find.ts` is empty at v0.84.1, and that proves nothing: pi does not sort
            // because **fd already did**, inside the binary find.ts spawns. Reading the wrapper and
            // not the tool it wraps is what dropped the sort entirely. It is reinstated after the
            // loop, on the bounded set — see the `FD_MAX_BUFFER_LENGTH` block. Note this bounds the
            // stream, not just the vector: dropping out of the loop closes the receiver and
            // `LocalFs::walk`'s producer task breaks on the send error (ops/local/fs.rs).
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
                        // fd NEVER fails a search over a filesystem error met while traversing.
                        // Its worker sends every `Err` from the `ignore` walker down the results
                        // channel and returns `WalkState::Continue` (fd 10.5.0
                        // `src/walk.rs:500-505`); the receiver prints it only under
                        // `--show-errors` (`:227-231`), which pi's argv does not pass
                        // (find.ts:234-267); and the exit code is `ExitCode::Success` regardless
                        // (`:282-292`). fd therefore exits 0 with empty stderr, pi's
                        // `if (code !== 0)` guard (find.ts:304) is never entered, and the paths fd
                        // did emit are returned as an ordinary success.
                        //
                        // There is NO `ignore::Error` variant fd treats as fatal — fd's arm is a
                        // catch-all that never inspects the variant. `WithPath{Io}` (permission
                        // denied, EIO, a stale mount), `WithDepth{Loop}` (symlink cycle),
                        // `Partial`/`WithLineNumber` (a malformed pattern in an ignore file) and
                        // bare `Io` from parent-ignore loading all take the same path. So this arm
                        // discriminates on nothing and simply keeps collecting.
                        //
                        // Swallowing here cannot hide a bad search root: the
                        // `metadata(&search_root)` probe above already rejects BOTH a
                        // missing root and one that exists but is not a directory, with
                        // fd's own two-line `[fd error]: …` message — see the note on
                        // that gate for why pi's `Path not found:` (find.ts:171, the
                        // `customOps.glob` branch) is NOT this tool's message. A root that
                        // exists but cannot be opened yields zero rows and falls through to
                        // "No files found matching pattern" below — which is exactly what pi
                        // answers on fd's empty stdout (find.ts:311-320). Do NOT gate this on
                        // "the walk produced no rows": pi has no such gate, because fd never
                        // gives it a non-zero code to gate on.
                        Some(Err(_)) => continue,
                        None => break,
                    }
                }
            }
        }

        // fd SORTS, and cyrup did not. `ReceiverBuffer::stop` runs the moment the cap fires
        // (fd 10.5.0 `walk.rs:220-224`) and, if the receiver is still BUFFERING, sorts before
        // emitting (`walk.rs:281-285`):
        //
        //     if self.mode == ReceiverMode::Buffering { self.buffer.sort(); self.stream()?; }
        //
        // It is still buffering while no more than `MAX_BUFFER_LENGTH` (1000, `walk.rs:125`)
        // results have arrived and the 100 ms `DEFAULT_MAX_BUFFER_TIME` (`walk.rs:127`) has not
        // expired. Pi passes `--max-results` with `DEFAULT_LIMIT = 1000` (find.ts:44, :165, :252),
        // and the receiver checks `buffer.len() > MAX_BUFFER_LENGTH` BEFORE incrementing
        // `num_results`, so on the 1000th entry the buffer holds exactly 1000 — not more — and the
        // cap fires while still buffering. Under pi's own default, fd's output is sorted.
        //
        // The 100 ms deadline has no analogue here: cyrup's walk is single-threaded with no
        // streaming mode, so the length condition is the whole test. On a tree slow enough that
        // fd's deadline beats the cap, fd streams in arrival order where this sorts — timing
        // dependent, unreproducible in-process, and the one genuine residual.
        //
        // `Path::cmp`, not `str::cmp`: fd orders by `self.path().cmp(other.path())`
        // (`dir_entry.rs:132-136`), which compares COMPONENTS. Byte comparison inverts the two
        // whenever a name holds a character below `/` — `.`, `-` and space all qualify, so
        // dotfiles and hyphenated names would sort wrongly. It also normalises away the trailing
        // `/` this function appends to directories, which byte comparison would not.
        //
        // This is NOT the full-tree `sort()`+`truncate()` rejected above: the walk is still
        // bounded at `limit`, so the SET is unchanged and only its order moves.
        const FD_MAX_BUFFER_LENGTH: usize = 1000;
        if results.len() <= FD_MAX_BUFFER_LENGTH {
            results.sort_by(|a, b| Path::new(a).cmp(Path::new(b)));
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
            notices.push(format!(
                "{} limit reached",
                format_size(crate::truncate::DEFAULT_MAX_BYTES)
            ));
        }
        if !notices.is_empty() {
            text.push_str(&format!("\n\n[{}]", notices.join(". ")));
        }

        // Pi adds `truncation` only when the byte cap fired and emits `details: undefined` when no
        // key is set (find.ts:326-344). Mirror that exactly.
        let result_limit_reached = if limit_reached { Some(limit) } else { None };
        let truncation = if t.info.truncated { Some(t.info) } else { None };
        let details = if truncation.is_some() || result_limit_reached.is_some() {
            serde_json::to_value(FindDetails {
                truncation,
                result_limit_reached,
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
