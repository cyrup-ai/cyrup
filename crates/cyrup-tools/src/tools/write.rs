//! `write` — atomic create-or-overwrite with parent-dir creation + per-file lock (R-03-015/016).

use crate::config::WriteOpts;
use crate::lock::FileMutationLocks;
use crate::ops::FsOps;
use crate::{error, path};
use cyrup_core::{CancelToken, Content, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct WriteInput {
    path: String,
    content: String,
}

pub struct WriteTool {
    fs: Arc<dyn FsOps>,
    locks: Arc<FileMutationLocks>,
    cwd: PathBuf,
    #[allow(dead_code)]
    opts: WriteOpts,
    params: serde_json::Value,
}

impl WriteTool {
    pub fn new(
        fs: Arc<dyn FsOps>,
        locks: Arc<FileMutationLocks>,
        cwd: PathBuf,
        opts: WriteOpts,
    ) -> Self {
        // Byte-for-byte Pi's TypeBox emission (write.ts:14-17): verbatim descriptions,
        // no `additionalProperties`.
        let params = serde_json::json!({
            "type": "object",
            "required": ["path", "content"],
            "properties": {
                "path": { "type": "string", "description": "Path to the file to write (relative or absolute)" },
                "content": { "type": "string", "description": "Content to write to the file" }
            }
        });
        Self { fs, locks, cwd, opts, params }
    }
}

#[async_trait::async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &str {
        "write"
    }
    /// TOOL-045 — pi declares `label` explicitly beside `name` on every built-in
    /// `ToolDefinition` and the two are equal for all seven (`write.ts:187-188` @v0.83.0). See
    /// [`super::ReadTool::label`] for why the trait default was not left to stand in.
    fn label(&self) -> Option<&str> {
        Some("write")
    }
    fn parameters(&self) -> &serde_json::Value {
        &self.params
    }
    // No `execution_mode` override (TOOL-006). Pi's `write` definition object is
    // `write.ts:192-198` — `name`, `label`, `description`, `promptSnippet`, `promptGuidelines`,
    // `parameters` — and declares no `executionMode`; `git grep -n executionMode` at v0.84.1 over
    // `core/tools/` and `core/extensions/` hits only the plumbing
    // (`tool-definition-wrapper.ts:16`/`:44`, `extensions/types.ts:477`), so no built-in sets it
    // and every one of them inherits the parallel default. Upstream's ONLY serialization for the
    // mutators is `withFileMutationQueue` (write.ts:208), which cyrup already provides per-file
    // via [`FileMutationLocks`] in `execute` below. Declaring `Sequential` here made
    // `cyrup-agent`'s `any_seq` (agent.rs:905-908) route the WHOLE batch — reads and greps
    // included — through `execute_sequential`.

    // Verbatim from Pi (write.ts:189-192).
    fn description(&self) -> &str {
        "Write content to a file. Creates the file if it doesn't exist, overwrites if it does. \
         Automatically creates parent directories."
    }
    fn prompt_snippet(&self) -> Option<&str> {
        Some("Create or overwrite files")
    }
    fn prompt_guidelines(&self) -> Vec<&str> {
        vec!["Use write only for new files or complete rewrites."]
    }

    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: serde_json::Value,
        cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let input: WriteInput =
            serde_json::from_value(params).map_err(|e| error::invalid(format!("write: {e}")))?;
        let abs = path::resolve_to_cwd(&input.path, &self.cwd);

        let _guard = self.locks.guard(&abs, &cancel).await?;
        if cancel.is_cancelled() {
            return Err(error::aborted());
        }

        let bytes = input.content.as_bytes();
        self.fs.write_in_place(&abs, bytes).await?;
        // Pi brackets the write on BOTH sides: `throwIfAborted()` runs before `ops.writeFile`
        // (write.ts:220) AND immediately after it (write.ts:224), before the success value is
        // built — `throwIfAborted` itself is defined at write.ts:213-215 and throws
        // `"Operation aborted"`. Present at the ported v0.83.0 too (`:219`). Without the second
        // check a cancel landing during the write yields `Successfully wrote N bytes` here while
        // pi reports an aborted tool error, so the transcript disagrees with the user's own
        // cancellation. Pi does NOT undo the write — only the RESULT is reported as aborted — so
        // this deliberately runs after `write_in_place` has already landed the bytes. The guard is
        // still held, matching pi's "keep the queue locked until the current operation has
        // settled" comment at write.ts:214-216.
        if cancel.is_cancelled() {
            return Err(error::aborted());
        }

        // Pi reports `content.length` — JS string length = UTF-16 code units — not the UTF-8 byte
        // count, and uses the verb "Successfully wrote" (write.ts:222). Match both exactly.
        let len_utf16 = input.content.encode_utf16().count();
        Ok(ToolResult {
            content: vec![Content::text(format!(
                "Successfully wrote {len_utf16} bytes to {}",
                input.path
            ))],
            // Pi declares `ToolDefinition<…, undefined>` and returns `details: undefined`
            // (write.ts:223) — it never emits write details. Mirror that with `None`.
            details: None,
            terminate: false,
            ..Default::default()
        })
    }
}
