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
        if let Some(serde_json::Value::String(s)) = obj.get("edits")
            && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(s) {
                obj.insert("edits".to_string(), parsed);
            }
        // legacy single-edit
        if !obj.contains_key("edits")
            && let (Some(old), Some(new)) = (obj.remove("oldText"), obj.remove("newText")) {
                obj.insert(
                    "edits".to_string(),
                    serde_json::json!([{ "oldText": old, "newText": new }]),
                );
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
            // Pi `validateEditInput` (edit.ts:120-125).
            return Err(error::invalid(
                "Edit tool input is invalid. edits must contain at least one replacement.",
            ));
        }

        let abs = path::resolve_to_cwd(&input.path, &self.cwd);
        let _guard = self.locks.guard(&abs, &cancel).await?;

        // R-03-021: validate writable before reading.
        // Pi: `Could not edit file: ${path}. ${errorMessage}.` — note the trailing period
        // (edit.ts:329). The `${errorMessage}` body itself (a Node errno string) is irreducible.
        self.fs.access(&abs, Access::ReadWrite).await.map_err(|e| {
            error::invalid(format!("Could not edit file: {}. {e}.", input.path))
        })?;

        let bytes = self.fs.read(&abs).await?;
        let raw = String::from_utf8_lossy(&bytes).into_owned();
        let (had_bom, body) = edit_diff::strip_bom(&raw);
        let ending = edit_diff::detect_line_ending(body);
        let norm = edit_diff::normalize_to_lf(body);

        // Exact-then-fuzzy multi-edit core (R-03-017, edit-diff.ts:304-366).
        let pairs: Vec<(String, String)> = input
            .edits
            .iter()
            .map(|e| (e.old_text.clone(), e.new_text.clone()))
            .collect();
        let applied = edit_diff::apply_edits_to_normalized_content(&norm, &pairs, &input.path)
            .map_err(|e| error::invalid(e.0))?;
        let new_body = applied.new_content;

        if cancel.is_cancelled() {
            return Err(error::aborted());
        }

        // Restore line endings + BOM (R-03-018).
        let restored = edit_diff::restore_line_endings(&new_body, ending);
        let final_text = if had_bom { format!("\u{feff}{restored}") } else { restored };
        self.fs.write_atomic(&abs, final_text.as_bytes()).await?;

        let (diff, first_changed_line) =
            edit_diff::generate_diff_string(&applied.base_content, &new_body);
        let patch = edit_diff::unified_patch(&input.path, &applied.base_content, &new_body);

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
    // Verbatim from Pi (edit.ts:296-308).
    fn description(&self) -> &str {
        "Edit a single file using exact text replacement. Every edits[].oldText must match a \
         unique, non-overlapping region of the original file. If two changes affect the same block \
         or nearby lines, merge them into one edit instead of emitting overlapping edits. Do not \
         include large unchanged regions just to connect distant changes."
    }
    fn prompt_snippet(&self) -> Option<&str> {
        Some(
            "Make precise file edits with exact text replacement, including multiple disjoint edits \
             in one call",
        )
    }
    fn prompt_guidelines(&self) -> &[&str] {
        &[
            "Use edit for precise changes (edits[].oldText must match exactly)",
            "When changing multiple separate locations in one file, use one edit call with multiple \
             entries in edits[] instead of multiple edit calls",
            "Each edits[].oldText is matched against the original file, not after earlier edits are \
             applied. Do not emit overlapping or nested edits. Merge nearby changes into one edit.",
            "Keep edits[].oldText as small as possible while still being unique in the file. Do not \
             pad with large unchanged regions.",
        ]
    }
}
