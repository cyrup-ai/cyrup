//! `write` — atomic create-or-overwrite with parent-dir creation + per-file lock (R-03-015/016).

use crate::config::WriteOpts;
use crate::details::WriteDetails;
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
        let params = serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path (relative to cwd or absolute)." },
                "content": { "type": "string", "description": "Full file contents to write." }
            },
            "required": ["path", "content"],
            "additionalProperties": false
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

        let n = bytes.len();
        Ok(ToolResult {
            content: vec![Content::text(format!("Wrote {n} bytes to {}", input.path))],
            details: serde_json::to_value(WriteDetails { bytes_written: n }).ok(),
            terminate: false,
        })
    }
}

impl ToolMeta for WriteTool {
    fn description(&self) -> &str {
        "Create or overwrite a file atomically (creating parent directories as needed)."
    }
    fn prompt_snippet(&self) -> Option<&str> {
        Some("write: create or overwrite a file with the given contents.")
    }
    fn prompt_guidelines(&self) -> &[&str] {
        &["Use `write` for new files or full rewrites; prefer `edit` for targeted changes."]
    }
}
