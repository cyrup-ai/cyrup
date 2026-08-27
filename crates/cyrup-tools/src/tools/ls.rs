//! `ls` — sorted directory listing including dotfiles (R-03-035/036, arch-03 §6.8). One-shot.

use crate::config::LsOpts;
use crate::details::LsDetails;
use crate::ops::FsOps;
use crate::truncate::{TruncOpts, format_size, truncate_head};
use crate::{error, path};
use cyrup_core::{CancelToken, Content, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct LsInput {
    path: Option<String>,
    // Pi's TypeBox `Type.Number` (ls.ts:16) carries no `integer` and no `minimum`, and Pi never
    // validates tool arguments at runtime, so `limit: 500.0` and `limit: -1` are inputs Pi accepts.
    // Modeling this as `usize` rejected the whole call at deserialization. See [`crate::jsnum`].
    limit: Option<f64>,
}

pub struct LsTool {
    fs: Arc<dyn FsOps>,
    cwd: PathBuf,
    opts: LsOpts,
    params: serde_json::Value,
}

impl LsTool {
    pub fn new(fs: Arc<dyn FsOps>, cwd: PathBuf, opts: LsOpts) -> Self {
        // Byte-for-byte Pi's TypeBox emission (ls.ts:14-17): verbatim descriptions, `type:"number"`,
        // no `minimum`, no `additionalProperties`, and — because BOTH properties are optional —
        // NO `required` key at all (TypeBox omits an empty `required`).
        let params = serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Directory to list (default: current directory)" },
                "limit": { "type": "number", "description": "Maximum number of entries to return (default: 500)" }
            }
        });
        Self {
            fs,
            cwd,
            opts,
            params,
        }
    }
}

#[async_trait::async_trait]
impl Tool for LsTool {
    fn name(&self) -> &str {
        "ls"
    }
    /// TOOL-045 — pi declares `label` explicitly beside `name` on every built-in
    /// `ToolDefinition` and the two are equal for all seven (`ls.ts:101-102` @v0.83.0). See
    /// [`super::ReadTool::label`] for why the trait default was not left to stand in.
    fn label(&self) -> Option<&str> {
        Some("ls")
    }
    fn parameters(&self) -> &serde_json::Value {
        &self.params
    }

    // Verbatim from Pi (ls.ts:103-104). DEFAULT_LIMIT=500, DEFAULT_MAX_BYTES/1024=50. Pi defines no
    // promptGuidelines for ls.
    fn description(&self) -> &str {
        "List directory contents. Returns entries sorted alphabetically, with '/' suffix for \
         directories. Includes dotfiles. Output is truncated to 500 entries or 50KB (whichever is \
         hit first)."
    }
    fn prompt_snippet(&self) -> Option<&str> {
        Some("List directory contents")
    }

    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: serde_json::Value,
        cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        // Pi's FIRST statement inside the executor, before `resolveToCwd`, before `ops.exists`,
        // before `ops.stat`, before `ops.readdir`:
        // `if (signal?.aborted) { reject(new Error("Operation aborted")); return; }` (ls.ts:119-122).
        // Without it an already-cancelled `ls` reported the WRONG outcome — `Path not found:` /
        // `Not a directory:` / `Cannot read directory:` on a bad path, or `(empty directory)` as a
        // SUCCESS on an empty one — instead of the abort the user asked for, after paying a wasted
        // `metadata`/`read_dir` pair. `ls.ts` has no runtime parameter validation, so "abort first,
        // parse second" is also pi's order. Same guard, same position, as the sibling
        // `find.rs:115-117`.
        if cancel.is_cancelled() {
            return Err(error::aborted());
        }

        let input: LsInput =
            serde_json::from_value(params).map_err(|e| error::invalid(format!("ls: {e}")))?;

        let abs = path::resolve_to_cwd(input.path.as_deref().unwrap_or("."), &self.cwd);

        // Pi: `const effectiveLimit = limit ?? DEFAULT_LIMIT` (ls.ts:130) — no clamp at all. A
        // negative or zero limit satisfies `results.length >= effectiveLimit` on the very first
        // iteration (ls.ts:161), so the loop collects nothing and Pi returns "(empty directory)"
        // before any notice is built; folding negatives to 0 reproduces that exactly. `??` is
        // null/undefined-only, so a JSON `null` takes the default. Sits above the abort window
        // because it is pure arithmetic on the already-parsed input — no I/O, nothing to cancel.
        let limit = input.limit.map_or(self.opts.limit, crate::jsnum::to_count);

        // ---- Pi's abort-listener window: ls.ts:124-125 (register) … ls.ts:178 (remove) ----
        // `const onAbort = () => reject(new Error("Operation aborted"));
        //  signal?.addEventListener("abort", onAbort, { once: true });`
        // rejects the OUTER promise the instant the signal fires, so upstream a cancel landing
        // mid-`readdir` or mid-stat-loop surfaces `Operation aborted` immediately even though
        // Node's `readdir` is not itself cancellable. The `FsOps` seam is likewise cancel-blind
        // (`metadata`/`read_dir` take no token, ops/mod.rs:368-369, and `LocalFs::read_dir` drains
        // `next_entry` to completion, ops/local/fs.rs:188-207), so the equivalent has to be built
        // here at the call site — exactly as `find.rs` and `grep.rs` build theirs, rather than by
        // widening the trait for every backend. Everything the listener covers goes in this block
        // and nothing else does: pi removes the listener at ls.ts:178, BEFORE the
        // `(empty directory)` resolve (ls.ts:180) and before truncation and notice building, so
        // once the entries are collected a cancel no longer changes the result.
        let listing = async {
            let meta = self
                .fs
                .metadata(&abs)
                .await
                // Pi: `Path not found: ${dirPath}` (ls.ts:134).
                .map_err(|_| error::not_found(format!("Path not found: {}", error::show(&abs))))?;
            if !meta.is_dir {
                // Pi: `Not a directory: ${dirPath}` (ls.ts:141).
                return Err(error::invalid(format!(
                    "Not a directory: {}",
                    error::show(&abs)
                )));
            }

            // Pi ls.ts:147-152:
            // ```
            // try { entries = await ops.readdir(dirPath); }
            // catch (e: any) { reject(new Error(`Cannot read directory: ${e.message}`)); return; }
            // ```
            // — a THIRD stable prefix beside `Path not found:` (ls.ts:134) and `Not a directory:`
            // (ls.ts:141) above, distinguishing "exists, is a directory, cannot be enumerated"
            // (mode `0300`, EIO, a permissions-stripped `.git/objects`) from the other two. The `?`
            // used to propagate `FsOps::read_dir`'s raw `"<path>: <io error>"` wrapper, which
            // carries none of the three prefixes. `read_dir` now builds its error with
            // `error::io_errno`, so `{e}` renders Node-shaped — leading with the errno code, as
            // `e.message` does upstream.
            let mut entries = self
                .fs
                .read_dir(&abs)
                .await
                .map_err(|e| error::invalid(format!("Cannot read directory: {e}")))?;

            // Pi: `entries.sort((a, b) => a.toLowerCase().localeCompare(b.toLowerCase()))`
            // (ls.ts:155) — case-insensitive, locale-aware Unicode collation (ICU-backed in the JS
            // engine). Rust std orders by Unicode scalar value, which diverges for
            // accented/punctuation-adjacent names (e.g. `é` collates near `e` under UCA but after
            // `z` by scalar value). `feruca` is a pure-Rust Unicode Collation Algorithm impl. We
            // mirror the JS engine's default `localeCompare` (CLDR root collation, "non-ignorable"
            // variable handling, so leading punctuation like a dotfile's `.` keeps a real primary
            // weight and sorts BEFORE letters — matching Node's `".dot".localeCompare("a.txt")
            // === -1`). `feruca`'s default `Collator` uses "shifted" handling, which would IGNORE
            // that dot; so we build a non-ignorable collator: `Collator::new(Tailoring::default()
            // /* CLDR Root */, false /* shifting */, true /* byte-value tiebreak */)`. We
            // lower-case both keys first to mirror Pi's `.toLowerCase()` pre-step exactly.
            let mut collator = feruca::Collator::new(feruca::Tailoring::default(), false, true);
            entries
                .sort_by(|a, b| collator.collate(&a.name.to_lowercase(), &b.name.to_lowercase()));

            let mut lines: Vec<String> = Vec::new();
            let mut limit_reached = false;
            for entry in &entries {
                // The `select!` below observes a cancel only while this future is PARKED on an
                // await; one that lands while the previous entry was being formatted is observed
                // here, on the next turn. Same belt-and-braces pairing as `find.rs:173-175`
                // sitting beside its `select!`.
                if cancel.is_cancelled() {
                    return Err(error::aborted());
                }
                if lines.len() >= limit {
                    limit_reached = true;
                    break;
                }
                match self.fs.metadata(&entry.path).await {
                    Ok(m) => {
                        if m.is_dir {
                            lines.push(format!("{}/", entry.name));
                        } else {
                            lines.push(entry.name.clone());
                        }
                    }
                    Err(_) => continue, // skip unstattable (R-03-035)
                }
            }
            Ok::<(Vec<String>, bool), ToolError>((lines, limit_reached))
        };

        let (lines, limit_reached) = tokio::select! {
            // `biased;` — without it `select!` polls its arms in RANDOM order, so with the token
            // already cancelled AND the listing future ready to make progress, the listing arm
            // would win roughly half the time and the tool would keep statting entries after Esc.
            // Pi's abort is deterministic on this edge (the `{once:true}` listener at ls.ts:124-125
            // rejects the instant the signal fires), so the cancel arm must be polled first.
            // `find.rs:183` carries the same `biased;` for the same reason; `grep.rs:394` is the
            // one site that still lacks it and is NOT the model to follow.
            biased;
            _ = cancel.cancelled() => return Err(error::aborted()),
            r = listing => r?,
        };

        if lines.is_empty() {
            return Ok(ToolResult {
                content: vec![Content::text("(empty directory)")],
                details: None,
                terminate: false,
                ..Default::default()
            });
        }

        let joined = lines.join("\n");
        let t = truncate_head(&joined, TruncOpts::bytes_only(self.opts.max_bytes));

        // Pi joins notices into ONE bracket (ls.ts:185-197); note: no "or refine pattern".
        let mut text = t.content.clone();
        let mut notices: Vec<String> = Vec::new();
        if limit_reached {
            notices.push(format!(
                "{limit} entries limit reached. Use limit={} for more",
                limit.saturating_mul(2)
            ));
        }
        if t.info.truncated {
            // Pi hardcodes `formatSize(DEFAULT_MAX_BYTES)` (ls.ts:192).
            notices.push(format!(
                "{} limit reached",
                format_size(crate::truncate::DEFAULT_MAX_BYTES)
            ));
        }
        if !notices.is_empty() {
            text.push_str(&format!("\n\n[{}]", notices.join(". ")));
        }

        // Pi adds `truncation` only when the byte cap fired and emits `details: undefined` when no
        // key is set (ls.ts:184-201). Mirror that exactly.
        let entry_limit_reached = if limit_reached { Some(limit) } else { None };
        let truncation = if t.info.truncated { Some(t.info) } else { None };
        let details = if truncation.is_some() || entry_limit_reached.is_some() {
            serde_json::to_value(LsDetails {
                truncation,
                entry_limit_reached,
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
mod collation_tests {
    // Ground truth captured from Node.js (the JS engine Pi runs on), reproducing ls.ts:150:
    //   const a = [".dot","a.txt","B.txt","zdir","é","e","z","Z","apple","Apple","2","10","1"];
    //   a.sort((x, y) => x.toLowerCase().localeCompare(y.toLowerCase()));
    //   // => [".dot","1","10","2","a.txt","apple","Apple","B.txt","e","é","z","Z","zdir"]
    // This pins the exact `feruca` configuration (non-ignorable variable handling) that matches the
    // engine's default `localeCompare`: leading punctuation (`.dot`) sorts BEFORE letters, accents
    // collate adjacent to their base letter (`e` < `é` < `z`), and case is a tertiary tiebreak.
    #[test]
    fn collation_matches_node_localecompare() {
        let mut names = vec![
            ".dot", "a.txt", "B.txt", "zdir", "é", "e", "z", "Z", "apple", "Apple", "2", "10", "1",
        ];
        let mut collator = feruca::Collator::new(feruca::Tailoring::default(), false, true);
        names.sort_by(|a, b| collator.collate(&a.to_lowercase(), &b.to_lowercase()));
        assert_eq!(
            names,
            vec![
                ".dot", "1", "10", "2", "a.txt", "apple", "Apple", "B.txt", "e", "é", "z", "Z",
                "zdir"
            ]
        );
    }
}
