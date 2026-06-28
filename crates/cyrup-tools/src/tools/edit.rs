//! `edit` — exact-match multi-edit against the original content, CRLF/BOM preserving, with diff +
//! unified patch (R-03-017…021, arch-03 §6.4). Per-file lock; no streaming.

use crate::config::EditOpts;
use crate::details::EditDetails;
use crate::lock::FileMutationLocks;
use crate::ops::{Access, FsOps};
use crate::tools::edit_diff;
use crate::{error, path, ToolMeta};
use cyrup_core::{CancelToken, Content, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct EditOp {
    old_text: String,
    new_text: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct EditInput {
    path: String,
    edits: Vec<EditOp>,
}

pub struct EditTool {
    fs: Arc<dyn FsOps>,
    locks: Arc<FileMutationLocks>,
    cwd: PathBuf,
    #[allow(dead_code)]
    opts: EditOpts,
    params: serde_json::Value,
}

impl EditTool {
    pub fn new(
        fs: Arc<dyn FsOps>,
        locks: Arc<FileMutationLocks>,
        cwd: PathBuf,
        opts: EditOpts,
    ) -> Self {
        let params = serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path (must exist)." },
                "edits": {
                    "type": "array",
                    "description": "Edits, each replacing a unique oldText with newText.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "oldText": { "type": "string" },
                            "newText": { "type": "string" }
                        },
                        "required": ["oldText", "newText"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["path", "edits"],
            "additionalProperties": false
        });
        Self { fs, locks, cwd, opts, params }
    }
}

/// Normalize legacy shapes into `{ path, edits: [...] }` (R-03-020):
/// - `edits` sent as a JSON string -> parse to array;
/// - top-level `oldText`/`newText` -> single-element `edits`.
fn normalize_args(mut raw: serde_json::Value) -> serde_json::Value {
    if let Some(obj) = raw.as_object_mut() {
        // edits-as-string
        if let Some(serde_json::Value::String(s)) = obj.get("edits") {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(s) {
                obj.insert("edits".to_string(), parsed);
            }
        }
        // legacy single-edit
        if !obj.contains_key("edits") {
            if let (Some(old), Some(new)) = (obj.remove("oldText"), obj.remove("newText")) {
                obj.insert(
                    "edits".to_string(),
                    serde_json::json!([{ "oldText": old, "newText": new }]),
                );
            }
        }
    }
    raw
}

#[async_trait::async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str {
        "edit"
    }
    fn parameters(&self) -> &serde_json::Value {
        &self.params
    }
    fn execution_mode(&self) -> cyrup_core::ExecMode {
        cyrup_core::ExecMode::Sequential
    }

    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: serde_json::Value,
        cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let params = normalize_args(params);
        let input: EditInput =
            serde_json::from_value(params).map_err(|e| error::invalid(format!("edit: {e}")))?;
        if input.edits.is_empty() {
            return Err(error::invalid("edit: at least one edit is required"));
        }

        let abs = path::resolve_to_cwd(&input.path, &self.cwd);
        let _guard = self.locks.guard(&abs, &cancel).await?;

        // R-03-021: validate writable before reading.
        self.fs.access(&abs, Access::ReadWrite).await.map_err(|e| {
            error::invalid(format!("Could not edit file: {}. {e}", input.path))
        })?;

        let bytes = self.fs.read(&abs).await?;
        let raw = String::from_utf8_lossy(&bytes).into_owned();
        let (had_bom, body) = edit_diff::strip_bom(&raw);
        let ending = edit_diff::detect_line_ending(body);
        let norm = edit_diff::normalize_to_lf(body);

        // Compute every edit's unique offset against the ORIGINAL (R-03-017).
        let mut ranges: Vec<(usize, usize, &str)> = Vec::new();
        for e in &input.edits {
            let mut indices = norm.match_indices(&e.old_text);
            let first = indices.next();
            let extra = indices.next();
            match (first, extra) {
                (None, _) => {
                    return Err(error::invalid(format!(
                        "edit: oldText not found in {}:\n{}",
                        input.path, e.old_text
                    )));
                }
                (Some(_), Some(_)) => {
                    return Err(error::invalid(format!(
                        "edit: oldText is not unique in {} (matches more than once):\n{}",
                        input.path, e.old_text
                    )));
                }
                (Some((start, matched)), None) => {
                    ranges.push((start, start + matched.len(), &e.new_text));
                }
            }
        }

        // Non-overlap check (R-03-017).
        ranges.sort_by_key(|r| r.0);
        let mut prev_end = 0usize;
        for (i, (start, end, _)) in ranges.iter().enumerate() {
            if i > 0 && *start < prev_end {
                return Err(error::invalid(
                    "edit: edits overlap; they must apply to disjoint regions",
                ));
            }
            prev_end = *end;
        }

        // Splice all edits against the original in ascending order.
        let mut new_body = String::with_capacity(norm.len());
        let mut cursor = 0usize;
        for (start, end, new_text) in &ranges {
            new_body.push_str(norm.get(cursor..*start).unwrap_or(""));
            new_body.push_str(new_text);
            cursor = *end;
        }
        new_body.push_str(norm.get(cursor..).unwrap_or(""));

        if cancel.is_cancelled() {
            return Err(error::aborted());
        }

        // Restore line endings + BOM (R-03-018).
        let restored = edit_diff::restore_line_endings(&new_body, ending);
        let final_text = if had_bom { format!("\u{feff}{restored}") } else { restored };
        self.fs.write_atomic(&abs, final_text.as_bytes()).await?;

        let diff = edit_diff::display_diff(&norm, &new_body);
        let patch = edit_diff::unified_patch(&input.path, &norm, &new_body);
        let first_changed_line = edit_diff::first_changed_line(&norm, &new_body);

        let count = input.edits.len();
        Ok(ToolResult {
            content: vec![Content::text(format!(
                "Successfully replaced {count} block(s) in {}.",
                input.path
            ))],
            details: serde_json::to_value(EditDetails { diff, patch, first_changed_line }).ok(),
            terminate: false,
        })
    }
}

impl ToolMeta for EditTool {
    fn description(&self) -> &str {
        "Apply exact-match edits to a file; each oldText must match exactly once."
    }
    fn prompt_snippet(&self) -> Option<&str> {
        Some("edit: replace unique text spans in a file.")
    }
    fn prompt_guidelines(&self) -> &[&str] {
        &["Use `edit` for targeted changes; include enough context so each oldText is unique."]
    }
}
