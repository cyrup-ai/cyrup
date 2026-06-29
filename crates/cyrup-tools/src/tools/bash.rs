//! `bash` — run a command in the cwd, stream combined stdout+stderr, tail-truncate the preview,
//! spill full output to a temp file, and kill the process tree on timeout/abort
//! (R-03-022…027, R-03-044, arch-03 §6.5).

use crate::config::{BashOpts, BashSpawnContext};
use crate::details::BashDetails;
use crate::ops::{shell_env, ExecSpec, ExitStatus, ProcOps, ShellConfig};
use crate::output::OutputAccumulator;
use crate::truncate::{format_size, truncate_tail, TruncOpts, Truncation, TruncatedBy};
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

        // Pi prepends the command prefix, then builds `{command, cwd, env: getShellEnv()}` and runs
        // it through the optional spawnHook before exec (bash.ts:294-295,141-144).
        let command = match &self.opts.command_prefix {
            Some(prefix) => format!("{prefix}\n{}", input.command),
            None => input.command.clone(),
        };
        let env = shell_env(self.opts.bin_dir.as_deref());
        let ctx = BashSpawnContext { command, cwd: self.cwd.clone(), env };
        let ctx = match &self.opts.spawn_hook {
            Some(hook) => hook(ctx),
            None => ctx,
        };

        // Resolve the shell per-exec, honoring an explicit settings `shellPath` (Pi's
        // `createLocalBashOperations` calls `getShellConfig(shellPath)` inside `exec`,
        // bash.ts:69); a missing custom path surfaces as the `Custom shell path not found` error.
        let shell = match self.opts.shell_path.as_deref() {
            Some(p) => ShellConfig::resolve(Some(p))?,
            None => self.shell.clone(),
        };

        let spec = ExecSpec { command: ctx.command, cwd: ctx.cwd, env: ctx.env, shell };

        let timeout = input.timeout.map(Duration::from_secs);
        let max_lines = self.opts.max_lines;
        let max_bytes = self.opts.max_bytes;

        let mut acc = OutputAccumulator::new("cyrup-bash", max_lines, max_bytes);
        let mut sink = on_update;

        // Pi emits an initial empty update before streaming (bash.ts:338-340).
        sink(ToolUpdate { content: vec![], details: None });

        let exec_result = {
            let acc_ref = &mut acc;
            let sink_ref = &mut sink;
            let mut last: Option<Instant> = None;
            let mut on_data = move |chunk: &[u8]| {
                acc_ref.append(chunk);
                // Leading-edge 100ms throttle (Pi BASH_UPDATE_THROTTLE_MS, bash.ts:158,323-336).
                // [CYRUP-DELTA]: Pi additionally schedules a *trailing* flush of the last dirty
                // snapshot via a timer; cyrup's `on_data` is a synchronous callback with no timer
                // task, so a sub-100ms final burst is instead delivered by the guaranteed final
                // settle update below — the settled content is identical.
                let due = last.is_none_or(|t| t.elapsed() >= Duration::from_millis(100));
                if due {
                    last = Some(Instant::now());
                    // Pi sends `snapshot.content` plus `details: { truncation?, fullOutputPath? }`
                    // (bash.ts:306-313). Build the same mid-stream snapshot here.
                    let snap = acc_ref.tail_string();
                    let preview = truncate_tail(&snap, TruncOpts::new(max_lines, max_bytes));
                    let truncated = acc_ref.is_truncated();
                    let total_lines = acc_ref.total_lines();
                    let total_bytes = acc_ref.total_bytes();
                    let full_path = acc_ref.snapshot_path();
                    let details = stream_details(
                        preview.info,
                        truncated,
                        total_lines,
                        total_bytes,
                        max_bytes,
                        full_path,
                    );
                    sink_ref(ToolUpdate {
                        content: vec![Content::text(preview.content)],
                        details,
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
        let last_line_bytes = acc.last_line_bytes();
        let full_path = acc.finalize(max_lines, max_bytes);
        let truncated = full_path.is_some();

        // Pi computes `truncatedBy` from the tail truncation, falling back to the overall
        // byte/line comparison when the tail itself wasn't truncated (output-accumulator.ts:96-99).
        let truncated_by = if truncated {
            t.info.truncated_by.or(if total_bytes > max_bytes {
                Some(crate::truncate::TruncatedBy::Bytes)
            } else {
                Some(crate::truncate::TruncatedBy::Lines)
            })
        } else {
            None
        };

        // Pi's last `onUpdate` (finishOutput→emitOutputUpdate) carries `snapshot.content` — the
        // preview WITHOUT the footer — while only the returned RESULT gets the footer (bash.ts:
        // 348-356,403-408). Capture the footer-less preview before appending the footer.
        let preview_content = t.content.clone();
        let mut text = t.content.clone();
        if let Some(path) = &full_path {
            let ps = path.to_string_lossy();
            let start_line = total_lines.saturating_sub(t.info.output_lines) + 1;
            let end_line = total_lines;
            // Footer wording matches Pi exactly (bash.ts:364-373).
            let footer = if t.info.last_line_partial {
                format!(
                    "[Showing last {} of line {end_line} (line is {}). Full output: {ps}]",
                    format_size(t.info.output_bytes),
                    format_size(last_line_bytes),
                )
            } else if truncated_by == Some(crate::truncate::TruncatedBy::Lines) {
                format!("[Showing lines {start_line}-{end_line} of {total_lines}. Full output: {ps}]")
            } else {
                format!(
                    "[Showing lines {start_line}-{end_line} of {total_lines} ({} limit). Full output: {ps}]",
                    format_size(max_bytes),
                )
            };
            if !text.is_empty() {
                text.push_str("\n\n");
            }
            text.push_str(&footer);
        }

        let mut info = t.info;
        info.total_lines = total_lines;
        info.total_bytes = total_bytes;
        info.truncated = truncated;
        info.truncated_by = truncated_by;
        // Pi sets `details` only when the output was truncated; otherwise it is `undefined`
        // (formatOutput, bash.ts:361-363). Mirror that on both the final update and the result.
        let full_output_path = full_path.as_ref().map(|p| p.to_string_lossy().into_owned());
        let details = if truncated {
            serde_json::to_value(BashDetails {
                truncation: Some(info),
                full_output_path: full_output_path.clone(),
            })
            .ok()
        } else {
            None
        };

        // Final settlement update (ignored if the sink is already settled, R-03-040). Pi sends the
        // footer-less preview content here with the same `details` shape it sends mid-stream.
        sink(ToolUpdate {
            content: vec![Content::text(preview_content)],
            details: details.clone(),
        });

        match status {
            // Pi treats a signal-killed process (exitCode null) as success with output preserved.
            ExitStatus::Exited(0) | ExitStatus::Signaled => {
                let body = if text.is_empty() { "(no output)".to_string() } else { text };
                Ok(ToolResult {
                    content: vec![Content::text(body)],
                    details,
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

/// Build the `details` payload for a mid-stream / final `onUpdate` (Pi bash.ts:306-313). The object
/// is always present; `truncation` is included only once a limit has fired, and `fullOutputPath`
/// only when the spill file exists — both serialized with `skip_serializing_if = None`, so the
/// not-yet-truncated case yields `{}`, byte-1:1 with Pi's `{truncation:undefined,fullOutputPath:
/// undefined}`.
fn stream_details(
    mut info: Truncation,
    truncated: bool,
    total_lines: usize,
    total_bytes: usize,
    max_bytes: usize,
    full_path: Option<PathBuf>,
) -> Option<serde_json::Value> {
    info.total_lines = total_lines;
    info.total_bytes = total_bytes;
    info.truncated = truncated;
    info.truncated_by = if truncated {
        info.truncated_by
            .or(if total_bytes > max_bytes { Some(TruncatedBy::Bytes) } else { Some(TruncatedBy::Lines) })
    } else {
        None
    };
    let truncation = if truncated { Some(info) } else { None };
    let full_output_path = full_path.map(|p| p.to_string_lossy().into_owned());
    serde_json::to_value(BashDetails { truncation, full_output_path }).ok()
}

impl ToolMeta for BashTool {
    // Verbatim from Pi (bash.ts:284-285). DEFAULT_MAX_LINES=2000, DEFAULT_MAX_BYTES/1024=50. Pi
    // defines no promptGuidelines for bash, so the trait default (`&[]`) is used.
    fn description(&self) -> &str {
        "Execute a bash command in the current working directory. Returns stdout and stderr. \
         Output is truncated to last 2000 lines or 50KB (whichever is hit first). If truncated, \
         full output is saved to a temp file. Optionally provide a timeout in seconds."
    }
    fn prompt_snippet(&self) -> Option<&str> {
        Some("Execute bash commands (ls, grep, find, etc.)")
    }
}
