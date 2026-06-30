//! `write` — atomic create-or-overwrite with parent-dir creation + per-file lock (R-03-015/016).

use crate::config::WriteOpts;
use crate::lock::FileMutationLocks;
use crate::ops::FsOps;
use crate::{error, path, ToolMeta};
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
        let input: WriteInput =
            serde_json::from_value(params).map_err(|e| error::invalid(format!("write: {e}")))?;
        let abs = path::resolve_to_cwd(&input.path, &self.cwd);

        let _guard = self.locks.guard(&abs, &cancel).await?;
        if cancel.is_cancelled() {
            return Err(error::aborted());
        }

        let bytes = input.content.as_bytes();
        self.fs.write_atomic(&abs, bytes).await?;

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
        })
    }
}

impl ToolMeta for WriteTool {
    // Verbatim from Pi (write.ts:189-192).
    fn description(&self) -> &str {
        "Write content to a file. Creates the file if it doesn't exist, overwrites if it does. \
         Automatically creates parent directories."
    }
    fn prompt_snippet(&self) -> Option<&str> {
        Some("Create or overwrite files")
    }
    fn prompt_guidelines(&self) -> &[&str] {
        &["Use write only for new files or complete rewrites."]
    }
}
