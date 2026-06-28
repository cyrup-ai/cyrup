//! `bash` — run a command in the cwd, stream combined stdout+stderr, tail-truncate the preview,
//! spill full output to a temp file, and kill the process tree on timeout/abort
//! (R-03-022…027, R-03-044, arch-03 §6.5).

use crate::config::BashOpts;
use crate::details::BashDetails;
use crate::ops::{ExecSpec, ExitStatus, ProcOps, ShellConfig};
use crate::output::OutputAccumulator;
use crate::truncate::{format_size, truncate_tail, TruncOpts};
use crate::{error, ToolMeta};
use cyrup_core::{
    CancelToken, Content, Tool, ToolCallId, ToolError, ToolResult, ToolUpdate, ToolUpdateSink,
};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct BashInput {
    command: String,
    timeout: Option<u64>,
}

pub struct BashTool {
    proc: Arc<dyn ProcOps>,
    shell: ShellConfig,
    cwd: PathBuf,
    opts: BashOpts,
    params: serde_json::Value,
}

impl BashTool {
    pub fn new(proc: Arc<dyn ProcOps>, shell: ShellConfig, cwd: PathBuf, opts: BashOpts) -> Self {
        let params = serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "Shell command to run in the cwd." },
                "timeout": { "type": "integer", "minimum": 1, "description": "Timeout in seconds (no default)." }
            },
            "required": ["command"],
            "additionalProperties": false
        });
        Self { proc, shell, cwd, opts, params }
    }
}

#[async_trait::async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }
    fn parameters(&self) -> &serde_json::Value {
        &self.params
    }

    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: serde_json::Value,
        cancel: CancelToken,
        on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let input: BashInput =
            serde_json::from_value(params).map_err(|e| error::invalid(format!("bash: {e}")))?;

        let command = match &self.opts.command_prefix {
            Some(prefix) => format!("{prefix}\n{}", input.command),
            None => input.command.clone(),
        };

        let spec = ExecSpec {
            command,
            cwd: self.cwd.clone(),
            env: Vec::new(),
            shell: self.shell.clone(),
        };

        let timeout = input.timeout.map(Duration::from_secs);
        let max_lines = self.opts.max_lines;
        let max_bytes = self.opts.max_bytes;

        let mut acc = OutputAccumulator::new("cyrup-bash", max_bytes);
        let mut sink = on_update;

        let exec_result = {
            let acc_ref = &mut acc;
            let sink_ref = &mut sink;
            let mut last: Option<Instant> = None;
            let mut on_data = move |chunk: &[u8]| {
                acc_ref.append(chunk);
                let due = last.is_none_or(|t| t.elapsed() >= Duration::from_millis(100));
                if due {
                    last = Some(Instant::now());
                    let snap = acc_ref.tail_string();
                    sink_ref(ToolUpdate {
                        content: vec![Content::text(snap)],
                        details: None,
                    });
                }
            };
            self.proc.exec(spec, cancel.clone(), timeout, &mut on_data).await
        };

        let status = exec_result?;

        let tail = acc.tail_string();
        let t = truncate_tail(&tail, TruncOpts::new(max_lines, max_bytes));
        let total_lines = acc.total_lines();
        let total_bytes = acc.total_bytes();
        let full_path = acc.finalize(max_lines, max_bytes);
        let truncated = full_path.is_some();

        let mut text = t.content.clone();
        if let Some(path) = &full_path {
            let ps = path.to_string_lossy();
            let footer = if t.info.last_line_partial {
                format!(
                    "[Showing last {} of line {total_lines} (line is {} bytes). Full output: {ps}]",
                    format_size(t.info.output_bytes),
                    total_bytes
                )
            } else if total_lines > max_lines {
                format!(
                    "[Showing last {} lines of {total_lines}. Full output: {ps}]",
                    t.info.output_lines
                )
            } else {
                format!(
                    "[Showing last {} of {} ({} limit). Full output: {ps}]",
                    format_size(t.info.output_bytes),
                    format_size(total_bytes),
                    format_size(max_bytes)
                )
            };
            if !text.is_empty() {
                text.push_str("\n\n");
            }
            text.push_str(&footer);
        }

        // Final settlement update (ignored if the sink is already settled, R-03-040).
        sink(ToolUpdate { content: vec![Content::text(text.clone())], details: None });

        let mut info = t.info;
        info.total_lines = total_lines;
        info.total_bytes = total_bytes;
        info.truncated = truncated;
        if !truncated {
            info.truncated_by = None;
        }
        let details = BashDetails {
            truncation: Some(info),
            full_output_path: full_path.as_ref().map(|p| p.to_string_lossy().into_owned()),
        };

        match status {
            ExitStatus::Exited(0) => {
                let body = if text.is_empty() { "(no output)".to_string() } else { text };
                Ok(ToolResult {
                    content: vec![Content::text(body)],
                    details: serde_json::to_value(details).ok(),
                    terminate: false,
                })
            }
            ExitStatus::Exited(code) => {
                Err(error::invalid(format!("{text}\n\nCommand exited with code {code}")))
            }
            ExitStatus::TimedOut => {
                let secs = input.timeout.unwrap_or(0);
                Err(error::invalid(format!("{text}\n\nCommand timed out after {secs} seconds")))
            }
            ExitStatus::Killed => Err(error::invalid(format!("{text}\n\nCommand aborted"))),
        }
    }
}

impl ToolMeta for BashTool {
    fn description(&self) -> &str {
        "Run a shell command in the working directory, streaming combined stdout+stderr."
    }
    fn prompt_snippet(&self) -> Option<&str> {
        Some("bash: run a shell command and stream its output.")
    }
    fn prompt_guidelines(&self) -> &[&str] {
        &["Use `bash` for commands and scripts; pass `timeout` for long-running commands."]
    }
}
