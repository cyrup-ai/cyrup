//! `edit` — exact-match multi-edit against the original content, CRLF/BOM preserving, with diff +
//! unified patch (R-03-017…021, arch-03 §6.4). Per-file lock; no streaming.

use crate::config::EditOpts;
use crate::details::EditDetails;
use crate::lock::FileMutationLocks;
use crate::ops::{Access, FsOps};
use crate::tools::edit_diff;
use crate::{error, path};
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
        // Byte-for-byte Pi's TypeBox emission (edit.ts:33-53): verbatim descriptions on every
        // property, and `additionalProperties:false` on BOTH the top object and the nested edit
        // object — `edit` is the ONLY tool whose source passes `{ additionalProperties: false }`.
        let params = serde_json::json!({
            "type": "object",
            "required": ["path", "edits"],
            "properties": {
                "path": { "type": "string", "description": "Path to the file to edit (relative or absolute)" },
                "edits": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "required": ["oldText", "newText"],
                        "properties": {
                            "oldText": { "type": "string", "description": "Exact text for one targeted replacement. It must be unique in the original file and must not overlap with any other edits[].oldText in the same call." },
                            "newText": { "type": "string", "description": "Replacement text for this targeted edit." }
                        },
                        "additionalProperties": false
                    },
                    "description": "One or more targeted replacements. Each edit is matched against the original file, not incrementally. Do not include overlapping or nested edits. If two changes touch the same block or nearby lines, merge them into one edit instead."
                }
            },
            "additionalProperties": false
        });
        Self { fs, locks, cwd, opts, params }
    }
}

/// Normalize legacy shapes into `{ path, edits: [...] }` (R-03-020), a 1:1 port of Pi
/// `prepareEditArguments` (edit.ts:94-118):
/// - `edits` sent as a JSON string -> parse, replacing only when it yields an array (edit.ts:102-107);
/// - whenever BOTH top-level `oldText`/`newText` are strings, APPEND `{oldText,newText}` to the
///   existing `edits` array (or a fresh one), regardless of whether `edits` is already present
///   (edit.ts:109-117: `const edits = Array.isArray(legacy.edits) ? [...legacy.edits] : []; edits.push(...)`).
///
/// The previous gate (`!obj.contains_key("edits")`) diverged from Pi: input
/// `{path, edits:[], oldText, newText}` made Pi succeed with one edit but made cyrup keep `edits:[]`
/// and fire the empty-array error, and `{edits:[{...}], oldText, newText}` had Pi append an extra
/// edit while cyrup ignored the pair.
fn normalize_args(mut raw: serde_json::Value) -> serde_json::Value {
    if let Some(obj) = raw.as_object_mut() {
        // edits-as-string -> parse, but only adopt the parsed value when it is an array
        // (Pi `if (Array.isArray(parsed)) args.edits = parsed`, edit.ts:104-106).
        if let Some(serde_json::Value::String(s)) = obj.get("edits")
            && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(s)
            && parsed.is_array()
        {
            obj.insert("edits".to_string(), parsed);
        }
        // legacy single-edit: append the pair whenever BOTH oldText and newText are strings
        // (Pi edit.ts:109-117). A non-string (or absent) oldText/newText leaves the args untouched.
        let both_strings = obj.get("oldText").is_some_and(serde_json::Value::is_string)
            && obj.get("newText").is_some_and(serde_json::Value::is_string);
        if both_strings {
            let old = obj.remove("oldText").unwrap_or(serde_json::Value::Null);
            let new = obj.remove("newText").unwrap_or(serde_json::Value::Null);
            let mut edits = match obj.get("edits") {
                Some(serde_json::Value::Array(a)) => a.clone(),
                _ => Vec::new(),
            };
            edits.push(serde_json::json!({ "oldText": old, "newText": new }));
            obj.insert("edits".to_string(), serde_json::Value::Array(edits));
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

    /// Pi wires the legacy-shape shim onto the tool DEFINITION as
    /// `prepareArguments: prepareEditArguments` (edit.ts:307), and the loop runs it BEFORE schema
    /// validation — `prepareToolCallArguments` then `validateToolArguments`
    /// (agent-loop.ts:596-598, 617-618). Without this override the identity default in
    /// `cyrup_core::Tool` applies and `{path, oldText, newText}` (or a stringified `edits`) is
    /// rejected by the `required:["path","edits"]` / `edits:{type:"array"}` schema in the agent
    /// preflight (`cyrup-agent/src/agent.rs`), so `normalize_args` inside [`Self::execute`] is
    /// never reached. `normalize_args` is idempotent, so `execute` may still call it for callers
    /// that bypass the preflight seam.
    async fn prepare_arguments(&self, args: serde_json::Value) -> serde_json::Value {
        normalize_args(args)
    }

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

    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: serde_json::Value,
        cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        // The preflight already applied this via [`Self::prepare_arguments`] (Pi's
        // `prepareArguments`, edit.ts:307); re-applying is a no-op on already-normalized args and
        // keeps direct `execute` callers that bypass the preflight seam working.
        let params = normalize_args(params);
        // Pi `validateEditInput` (edit.ts:120-125) runs FIRST and rejects ANY shape where `edits`
        // is not a non-empty array — missing, non-array, or empty — with this exact literal. This
        // precedes serde deserialization so a malformed `edits` surfaces Pi's message rather than a
        // serde type error.
        let edits_ok = params
            .get("edits")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|a| !a.is_empty());
        if !edits_ok {
            return Err(error::invalid(
                "Edit tool input is invalid. edits must contain at least one replacement.",
            ));
        }
        let input: EditInput =
            serde_json::from_value(params).map_err(|e| error::invalid(format!("edit: {e}")))?;

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
        self.fs.write_in_place(&abs, final_text.as_bytes()).await?;

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
            ..Default::default()
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::normalize_args;
    use serde_json::json;

    /// Byte-diff vs Pi `prepareEditArguments` (edit.ts:109-117): a legacy `{oldText,newText}` pair
    /// is APPENDED to an existing `edits` array, regardless of whether `edits` is present. Pi:
    /// `const edits = Array.isArray(legacy.edits) ? [...legacy.edits] : []; edits.push({oldText,newText})`.
    #[test]
    fn legacy_pair_appends_into_existing_empty_array() {
        // `{path, edits:[], oldText, newText}` — the regression input. Pi yields exactly one edit
        // (the appended pair); the old `!contains_key("edits")` gate left `edits:[]` and would have
        // fired the empty-array error.
        let out = normalize_args(json!({
            "path": "f.txt",
            "edits": [],
            "oldText": "a",
            "newText": "b"
        }));
        assert_eq!(
            out,
            json!({ "path": "f.txt", "edits": [{ "oldText": "a", "newText": "b" }] })
        );
        // `oldText`/`newText` are stripped from the result (Pi's `{ ...rest, edits }` excludes them).
        assert!(out.get("oldText").is_none());
        assert!(out.get("newText").is_none());
    }

    /// Byte-diff vs Pi: a populated `edits` array PLUS a legacy pair yields the original edits with
    /// the pair appended (edit.ts:114-115 `[...legacy.edits]` then `push`).
    #[test]
    fn legacy_pair_appends_after_existing_edits() {
        let out = normalize_args(json!({
            "path": "f.txt",
            "edits": [{ "oldText": "x", "newText": "y" }],
            "oldText": "a",
            "newText": "b"
        }));
        assert_eq!(
            out,
            json!({
                "path": "f.txt",
                "edits": [
                    { "oldText": "x", "newText": "y" },
                    { "oldText": "a", "newText": "b" }
                ]
            })
        );
    }

    /// No `edits` key + a legacy pair builds a fresh single-element array (the common shorthand).
    #[test]
    fn legacy_pair_without_edits_builds_single_element_array() {
        let out = normalize_args(json!({ "path": "f.txt", "oldText": "a", "newText": "b" }));
        assert_eq!(
            out,
            json!({ "path": "f.txt", "edits": [{ "oldText": "a", "newText": "b" }] })
        );
    }

    /// A non-string `oldText`/`newText` leaves the args untouched (Pi edit.ts:110-112 early return).
    #[test]
    fn non_string_pair_is_left_untouched() {
        let out = normalize_args(json!({ "path": "f.txt", "oldText": 1, "newText": "b" }));
        assert_eq!(out, json!({ "path": "f.txt", "oldText": 1, "newText": "b" }));
    }

    /// `edits` as a JSON string is parsed to an array (Pi edit.ts:102-107), and a legacy pair then
    /// appends onto the parsed array.
    #[test]
    fn edits_string_parses_then_pair_appends() {
        let out = normalize_args(json!({
            "path": "f.txt",
            "edits": "[{\"oldText\":\"x\",\"newText\":\"y\"}]",
            "oldText": "a",
            "newText": "b"
        }));
        assert_eq!(
            out,
            json!({
                "path": "f.txt",
                "edits": [
                    { "oldText": "x", "newText": "y" },
                    { "oldText": "a", "newText": "b" }
                ]
            })
        );
    }
}
