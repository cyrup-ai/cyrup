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
use std::time::Duration;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct BashInput {
    command: String,
    // Pi's TypeBox `Type.Number` (bash.ts:42) is a JS float measured in SECONDS, so `timeout:2.5`
    // must deserialize. Modeling this as `u64` (the old cyrup type) rejected any fractional value.
    timeout: Option<f64>,
}

/// Pi's `MAX_TIMEOUT_MS` (bash.ts:24): the 32-bit `setTimeout` ceiling.
const MAX_TIMEOUT_MS: f64 = 2_147_483_647.0;

/// Port of Pi's `resolveTimeoutMs` (bash.ts:27-38), called at the top of `exec` (bash.ts:85). The
/// input is a count of SECONDS. `None` stays `None` (no default timeout). A non-finite or
/// non-positive value throws Pi's exact `"Invalid timeout: must be a finite number of seconds"`;
/// a value whose millisecond form exceeds the 32-bit ceiling throws Pi's exact
/// `"Invalid timeout: maximum is 2147483.647 seconds"` (`${MAX_TIMEOUT_MS / 1000}`). Otherwise the
/// seconds are converted to a `Duration` of `round(secs * 1000)` ms.
fn resolve_timeout_ms(timeout: Option<f64>) -> Result<Option<Duration>, ToolError> {
    let Some(secs) = timeout else { return Ok(None) };
    if !secs.is_finite() || secs <= 0.0 {
        return Err(error::invalid("Invalid timeout: must be a finite number of seconds"));
    }
    let timeout_ms = secs * 1000.0;
    if timeout_ms > MAX_TIMEOUT_MS {
        return Err(error::invalid("Invalid timeout: maximum is 2147483.647 seconds"));
    }
    // `timeout_ms` is finite and in `(0, 2_147_483_647]`, so `round() as u64` is exact and total.
    Ok(Some(Duration::from_millis(timeout_ms.round() as u64)))
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
        // Byte-for-byte Pi's TypeBox emission (bash.ts:24-27): verbatim descriptions,
        // `type:"number"`, no `minimum`, no `additionalProperties`.
        let params = serde_json::json!({
            "type": "object",
            "required": ["command"],
            "properties": {
                "command": { "type": "string", "description": "Bash command to execute" },
                "timeout": { "type": "number", "description": "Timeout in seconds (optional, no default timeout)" }
            }
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

        let max_lines = self.opts.max_lines;
        let max_bytes = self.opts.max_bytes;

        let mut acc = OutputAccumulator::new("cyrup-bash", max_lines, max_bytes);
        let mut sink = on_update;

        // Pi emits an initial empty update before streaming (bash.ts:355-357). This happens BEFORE
        // `ops.exec` runs `resolveTimeoutMs` (bash.ts:400,85), so an invalid timeout surfaces AFTER
        // this empty update — emit it first, then validate.
        sink(ToolUpdate { content: vec![], details: None, terminate: None });

        // Pi's `resolveTimeoutMs` (bash.ts:85): validate the seconds and convert to a `Duration`.
        // An invalid value reaches Pi's catch as a plain `Error` that matches neither `"aborted"`
        // nor `"timeout:"`, so it is re-thrown verbatim via `throw err` (bash.ts:417) with no status
        // appended — mirror that by returning the raw error here.
        let timeout = resolve_timeout_ms(input.timeout)?;

        // Pi debounces mid-stream output updates with a 100ms throttle that has BOTH a leading edge
        // AND a scheduled TRAILING-edge `setTimeout` flush (`scheduleOutputUpdate`, bash.ts:158,
        // 302-336): a sub-threshold burst that arrives just after a leading emit is still flushed
        // ~100ms later, MID-STREAM, rather than waiting for the final settle. The `ProcOps::exec`
        // seam delivers data through a SYNCHRONOUS `on_data` callback with no timer, so — without
        // changing that cross-crate seam — each chunk is forwarded over an unbounded channel and a
        // concurrent tokio "flusher" runs alongside `exec`, owning the throttle timer and emitting
        // with the exact leading+trailing cadence Pi uses (`emitOutputUpdate`/`clearUpdateTimer`,
        // bash.ts:302-335). The channel closes when `exec` returns (its sender is dropped), which
        // ends the flusher; the final settle update below is Pi's `finishOutput` flush (bash.ts:
        // 348-356).
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();

        let exec_fut = async move {
            let mut on_data = move |chunk: &[u8]| {
                // A closed receiver (flusher already gone) is ignored: the bytes are still part of
                // the final snapshot, exactly like Pi dropping a late `onUpdate`.
                let _ = tx.send(chunk.to_vec());
            };
            let result = self.proc.exec(spec, cancel, timeout, &mut on_data).await;
            // Drop the sender so the flusher's `recv()` returns `None` and it settles.
            drop(on_data);
            result
        };

        let flush_fut = async {
            let throttle = Duration::from_millis(100);
            let mut last_emit: Option<tokio::time::Instant> = None;
            let mut dirty = false;
            // The single pending trailing-edge timer (Pi `updateTimer`, bash.ts:298,332).
            let mut deadline: Option<tokio::time::Instant> = None;
            loop {
                let timer_deadline = deadline;
                tokio::select! {
                    biased;
                    maybe = rx.recv() => match maybe {
                        Some(chunk) => {
                            acc.append(&chunk);
                            dirty = true;
                            // Pi: `delay = THROTTLE - (now - lastUpdateAt)`; `delay <= 0` ⇒ leading
                            // edge (emit immediately), else arm ONE trailing timer and coalesce
                            // further chunks into it (`updateTimer ??=`, bash.ts:323-335).
                            let due = last_emit.is_none_or(|t| t.elapsed() >= throttle);
                            if due {
                                deadline = None;
                                flush_update(
                                    &mut acc, &mut sink, &mut dirty, &mut last_emit, max_lines,
                                    max_bytes,
                                );
                            } else if deadline.is_none() {
                                deadline = last_emit.map(|t| t + throttle);
                            }
                        }
                        None => break,
                    },
                    _ = async move {
                        match timer_deadline {
                            Some(d) => tokio::time::sleep_until(d).await,
                            None => std::future::pending::<()>().await,
                        }
                    } => {
                        // Trailing edge fired: flush the accumulated dirty snapshot (Pi's
                        // `updateTimer` callback → `emitOutputUpdate`, bash.ts:332-335).
                        deadline = None;
                        flush_update(
                            &mut acc, &mut sink, &mut dirty, &mut last_emit, max_lines, max_bytes,
                        );
                    }
                }
            }
        };

        let (exec_result, ()) = tokio::join!(exec_fut, flush_fut);
        let status = exec_result?;

        // Flush the streaming decoder before reading final totals (Pi `output.finish()` precedes the
        // snapshot, output-accumulator.ts:80-89), so a trailing incomplete UTF-8 sequence counts.
        acc.finish();
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
                // Pi hardcodes `formatSize(DEFAULT_MAX_BYTES)` in this footer (bash.ts:372) even
                // though bash's truncation point itself is configurable. Mirror the constant.
                format!(
                    "[Showing lines {start_line}-{end_line} of {total_lines} ({} limit). Full output: {ps}]",
                    format_size(crate::truncate::DEFAULT_MAX_BYTES),
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
            terminate: None,
        });

        match status {
            // Pi treats a signal-killed process (exitCode null) as success with output preserved.
            // Both this arm and the non-zero-exit arm go through `formatOutput`, whose `emptyText`
            // defaults to `"(no output)"` (bash.ts:357,375).
            ExitStatus::Exited(0) | ExitStatus::Signaled => {
                let body = if text.is_empty() { "(no output)".to_string() } else { text };
                Ok(ToolResult {
                    content: vec![Content::text(body)],
                    details,
                    terminate: false,
                })
            }
            // Non-zero exit: `formatOutput(snapshot)` uses the `"(no output)"` default for empty
            // output, then `appendStatus` joins it (bash.ts:404-406).
            ExitStatus::Exited(code) => {
                let body = if text.is_empty() { "(no output)".to_string() } else { text };
                Err(error::invalid(append_status(&body, &format!("Command exited with code {code}"))))
            }
            // Catch path (abort/timeout): `formatOutput(snapshot, "")` — `emptyText` is `""`, so an
            // empty output yields just the status with NO leading `\n\n` (bash.ts:375,388-396).
            ExitStatus::TimedOut => {
                // Pi throws `timeout:${timeout}` (bash.ts:138) then prints the ORIGINAL seconds
                // value in `Command timed out after ${timeoutSecs} seconds` (bash.ts:414-415). Rust
                // `f64` Display matches JS number stringification (`1.0`→"1", `2.5`→"2.5").
                let secs = input.timeout.unwrap_or(0.0);
                Err(error::invalid(append_status(&text, &format!("Command timed out after {secs} seconds"))))
            }
            ExitStatus::Killed => Err(error::invalid(append_status(&text, "Command aborted"))),
        }
    }
}

/// Pi's `appendStatus = (text, status) => ${text ? `${text}\n\n` : ""}${status}` (bash.ts:377):
/// the `\n\n` separator is inserted ONLY when there is preceding text, so an empty body produces
/// the bare status with no leading newlines.
fn append_status(text: &str, status: &str) -> String {
    if text.is_empty() {
        status.to_string()
    } else {
        format!("{text}\n\n{status}")
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

/// Build a mid-stream `onUpdate` payload from the live accumulator (Pi `emitOutputUpdate`'s
/// `snapshot` + `onUpdate({ content, details })`, bash.ts:306-313): `snapshot.content` plus the
/// `{ truncation?, fullOutputPath? }` details shape.
fn build_stream_update(
    acc: &mut OutputAccumulator,
    max_lines: usize,
    max_bytes: usize,
) -> ToolUpdate {
    let snap = acc.tail_string();
    let preview = truncate_tail(&snap, TruncOpts::new(max_lines, max_bytes));
    let truncated = acc.is_truncated();
    let total_lines = acc.total_lines();
    let total_bytes = acc.total_bytes();
    let full_path = acc.snapshot_path();
    let details =
        stream_details(preview.info, truncated, total_lines, total_bytes, max_bytes, full_path);
    ToolUpdate { content: vec![Content::text(preview.content)], details, terminate: None }
}

/// Pi's `emitOutputUpdate` (bash.ts:302-314): a no-op unless something is dirty; otherwise clear the
/// dirty flag, stamp `lastUpdateAt`, and emit the current snapshot. The `last_emit` stamp drives the
/// next throttle window so the leading/trailing cadence matches Pi 1:1.
fn flush_update(
    acc: &mut OutputAccumulator,
    sink: &mut ToolUpdateSink,
    dirty: &mut bool,
    last_emit: &mut Option<tokio::time::Instant>,
    max_lines: usize,
    max_bytes: usize,
) {
    if !*dirty {
        return;
    }
    *dirty = false;
    *last_emit = Some(tokio::time::Instant::now());
    sink(build_stream_update(acc, max_lines, max_bytes));
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
