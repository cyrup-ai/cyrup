//! `ls` — sorted directory listing including dotfiles (R-03-035/036, arch-03 §6.8). One-shot.

use crate::config::LsOpts;
use crate::details::LsDetails;
use crate::ops::FsOps;
use crate::truncate::{format_size, truncate_head, TruncOpts};
use crate::{error, path};
use cyrup_core::{CancelToken, Content, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct LsInput {
    path: Option<String>,
    limit: Option<usize>,
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
        Self { fs, cwd, opts, params }
    }
}

#[async_trait::async_trait]
impl Tool for LsTool {
    fn name(&self) -> &str {
        "ls"
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
        let input: LsInput =
            serde_json::from_value(params).map_err(|e| error::invalid(format!("ls: {e}")))?;

        let abs = path::resolve_to_cwd(input.path.as_deref().unwrap_or("."), &self.cwd);
        let meta = self
            .fs
            .metadata(&abs)
            .await
            // Pi: `Path not found: ${dirPath}` (ls.ts:129).
            .map_err(|_| error::not_found(format!("Path not found: {}", error::show(&abs))))?;
        if !meta.is_dir {
            return Err(error::invalid(format!("Not a directory: {}", error::show(&abs))));
        }

        let mut entries = self.fs.read_dir(&abs).await?;
        // Pi: `entries.sort((a, b) => a.toLowerCase().localeCompare(b.toLowerCase()))` (ls.ts:150) —
        // case-insensitive, locale-aware Unicode collation (ICU-backed in the JS engine). Rust std
        // orders by Unicode scalar value, which diverges for accented/punctuation-adjacent names
        // (e.g. `é` collates near `e` under UCA but after `z` by scalar value). `feruca` is a
        // pure-Rust Unicode Collation Algorithm impl. We mirror the JS engine's default
        // `localeCompare` (CLDR root collation, "non-ignorable" variable handling, so leading
        // punctuation like a dotfile's `.` keeps a real primary weight and sorts BEFORE letters —
        // matching Node's `".dot".localeCompare("a.txt") === -1`). `feruca`'s default `Collator`
        // uses "shifted" handling, which would IGNORE that dot; so we build a non-ignorable
        // collator: `Collator::new(Tailoring::default() /* CLDR Root */, false /* shifting */,
        // true /* byte-value tiebreak */)`. We lower-case both keys first to mirror Pi's
        // `.toLowerCase()` pre-step exactly.
        let mut collator =
            feruca::Collator::new(feruca::Tailoring::default(), false, true);
        entries.sort_by(|a, b| collator.collate(&a.name.to_lowercase(), &b.name.to_lowercase()));

        let limit = input.limit.unwrap_or(self.opts.limit);
        let mut lines: Vec<String> = Vec::new();
        let mut limit_reached = false;
        for entry in &entries {
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
            notices.push(format!("{} limit reached", format_size(crate::truncate::DEFAULT_MAX_BYTES)));
        }
        if !notices.is_empty() {
            text.push_str(&format!("\n\n[{}]", notices.join(". ")));
        }

        // Pi adds `truncation` only when the byte cap fired and emits `details: undefined` when no
        // key is set (ls.ts:184-201). Mirror that exactly.
        let entry_limit_reached = if limit_reached { Some(limit) } else { None };
        let truncation = if t.info.truncated { Some(t.info) } else { None };
        let details = if truncation.is_some() || entry_limit_reached.is_some() {
            serde_json::to_value(LsDetails { truncation, entry_limit_reached }).ok()
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
