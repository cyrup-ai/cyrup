//! SUBA-074 stage 2 — `runExternalCli` (`pi-subagents/src/runs/shared/external-cli-runner.ts:153-429`
//! @v0.64.0): spawn the foreign process, feed the prompt down its single delivery channel, drain
//! both streams under bounded logs and bounded tails, run the adapter's JSONL parser over stdout,
//! and tear the whole process TREE down on a deadline or a stop.
//!
//! Two pieces of cyrup machinery replace upstream's own, and both are stronger:
//!
//! * **Process-tree teardown.** Upstream scrapes `ps` for process-group members
//!   (`runs/background/owned-process-tree.ts:24-56`). This crate already sets
//!   `Command::process_group(0)` on children it owns and signals the negated pgid
//!   (`crate::spawn::signal`), so `terminate_on_timeout`'s SIGTERM→SIGKILL ladder reaches the
//!   descendants directly with no scraping and no race.
//! * **The deadline/cancel race.** `biased` select with the exit arm first, exactly as
//!   [`crate::exec::acceptance::model::verify::run`] does and for the same reason: an unbiased
//!   select whose exit arm and cancel arm are both ready picks at random, so a process that had
//!   already finished when the token fired would report "stopped" about half the time.

use std::path::{Path, PathBuf};

use tokio::io::AsyncWriteExt;

use super::adapters::AdapterParser;
use super::framing::{
    BoundedLog, ByteTail, LineEvent, LineSplitter, MAX_ERROR_TAIL_BYTES, MAX_OUTPUT_TAIL_BYTES,
    MAX_PARSER_ERROR_BYTES, ParserTerminal, StreamLimits,
};
use super::prompt::PreparedPrompt;
use crate::runner::status::ExternalProcessStatus;

/// The terminal outcome of one foreign-process run — upstream's `ExternalCliRunResult` (`:62-72`)
/// minus the fields whose only consumer is the deferred adapters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalCliRunOutcome {
    /// The delivered output: the parser's terminal output when it completed, else the stdout tail.
    pub output: String,
    /// `1` for a timeout, a stop, a spawn failure or a parser failure; else the process's own code.
    pub exit_code: i32,
    /// The failure text, `None` on success.
    pub error: Option<String>,
    /// The run hit its wall-clock deadline.
    pub timed_out: bool,
    /// The run was stopped by the caller.
    pub stopped: bool,
    /// The killing signal's name, when the process died by signal.
    pub process_signal: Option<String>,
    /// The process receipt.
    pub external_process: ExternalProcessStatus,
}

/// Everything the process runner needs that is not part of the launch plan.
#[derive(Debug)]
pub struct ExternalCliRunInput<'a> {
    /// The child's working directory.
    pub cwd: PathBuf,
    /// Where the two bounded stream logs go (upstream's `asyncDir`).
    pub log_dir: PathBuf,
    /// The flat step index, which names the logs (`external-<index>.stdout.log`).
    pub step_index: usize,
    /// The wall-clock deadline, if the run has one.
    pub deadline: Option<tokio::time::Instant>,
    /// Fires a stop (upstream's `registerStop`).
    pub stop: &'a cyrup_core::CancelToken,
    /// `input.timeoutMessage ?? "Subagent timed out."` (`:404`).
    pub timeout_message: String,
    /// `input.stopMessage ?? "Subagent stopped by user."` (`:402`).
    pub stop_message: String,
}

/// The launch half the runner consumes: what to spawn and how to read it.
#[derive(Debug)]
pub struct ExternalCliProcessPlan {
    /// The binary to execute — the preflight-resolved path when there was one, else the command.
    pub program: PathBuf,
    /// The complete argv.
    pub args: Vec<String>,
    /// The child's environment, or `None` to inherit this process's.
    pub env: Option<std::collections::BTreeMap<String, String>>,
    /// The adapter's stream parser, when it has one.
    pub parser: Option<AdapterParser>,
    /// The bounded-stream ceilings.
    pub limits: StreamLimits,
    /// An adapter-owned final-output artifact, recorded on the receipt.
    pub final_output_path: Option<PathBuf>,
}

/// The pre-spawn failure shape (`:211-224`): exit 1, the failure text, and a receipt that still
/// names the log paths so a caller can see there was nothing to read.
fn pre_spawn_outcome(
    error: String,
    started_at: i64,
    stdout_path: &Path,
    stderr_path: &Path,
    final_output_path: Option<&Path>,
) -> ExternalCliRunOutcome {
    let ended_at = crate::time::now_epoch_millis();
    ExternalCliRunOutcome {
        output: String::new(),
        exit_code: 1,
        error: Some(error),
        timed_out: false,
        stopped: false,
        process_signal: None,
        external_process: ExternalProcessStatus {
            started_at,
            ended_at: Some(ended_at),
            duration_ms: Some(ended_at - started_at),
            exit_code: Some(1),
            stdout_path: stdout_path.display().to_string(),
            stderr_path: stderr_path.display().to_string(),
            final_output_path: final_output_path.map(|path| path.display().to_string()),
            ..ExternalProcessStatus::default()
        },
    }
}

/// Run one external CLI to completion.
///
/// `prompt` is the already-prepared delivery: its [`PreparedPrompt::stdin_payload`] decides whether
/// anything is written to stdin, and its `Drop` removes any temporary paths it created — including
/// on the pre-spawn error path below, which is the one upstream's `cleanupTemporaryPaths` is
/// easiest to forget on.
pub async fn run_external_cli_process(
    mut plan: ExternalCliProcessPlan,
    prompt: &PreparedPrompt,
    input: &ExternalCliRunInput<'_>,
) -> ExternalCliRunOutcome {
    let started_at = crate::time::now_epoch_millis();
    let stdout_path = input
        .log_dir
        .join(format!("external-{}.stdout.log", input.step_index));
    let stderr_path = input
        .log_dir
        .join(format!("external-{}.stderr.log", input.step_index));
    if let Err(error) = std::fs::create_dir_all(&input.log_dir) {
        return pre_spawn_outcome(
            error.to_string(),
            started_at,
            &stdout_path,
            &stderr_path,
            plan.final_output_path.as_deref(),
        );
    }

    let mut command = tokio::process::Command::new(&plan.program);
    command.args(&plan.args);
    command.current_dir(&input.cwd);
    command.stdin(std::process::Stdio::piped());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    if let Some(env) = &plan.env {
        // The ONE place this crate calls `env_clear`. `crate::spawn::ChildSpawnSpec` documents the
        // opposite rule for the native subagent child, and that rule is right there and wrong here:
        // a foreign CLI must see the adapter's allowlist projection and nothing else. See
        // [`super::env`] for the argument.
        command.env_clear();
        command.envs(env);
    }
    #[cfg(unix)]
    {
        // The child leads its own process group, so `crate::spawn::signal`'s ladder can signal
        // `kill(-pgid, …)` and reach the descendants the foreign agent spawned.
        command.process_group(0);
    }

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return pre_spawn_outcome(
                error.to_string(),
                started_at,
                &stdout_path,
                &stderr_path,
                plan.final_output_path.as_deref(),
            );
        }
    };
    let pid = child.id();

    // Deliver the prompt down its single channel and close stdin (`:363-364`). A write failure is
    // swallowed for the same reason upstream installs a no-op `stdin.on("error")`: a one-shot CLI
    // that exits before reading its input is a normal outcome, not a runner failure.
    if let Some(mut stdin) = child.stdin.take() {
        if let Some(payload) = prompt.stdin_payload() {
            let _ = stdin.write_all(payload.as_bytes()).await;
        }
        let _ = stdin.shutdown().await;
    }

    let mut stdout_log = BoundedLog::create(&stdout_path, plan.limits.stdout_log_bytes);
    let mut stderr_log = BoundedLog::create(&stderr_path, plan.limits.stderr_log_bytes);
    let mut stdout_tail = ByteTail::new(MAX_OUTPUT_TAIL_BYTES);
    let mut stderr_tail = ByteTail::new(MAX_ERROR_TAIL_BYTES);
    let mut splitter = LineSplitter::new(plan.limits);
    let mut parser_error: Option<String> = None;

    let (tx, mut rx) = tokio::sync::mpsc::channel::<(bool, Vec<u8>)>(64);
    if let Some(pipe) = child.stdout.take() {
        let tx = tx.clone();
        tokio::spawn(pump(pipe, tx, true));
    }
    if let Some(pipe) = child.stderr.take() {
        let tx = tx.clone();
        tokio::spawn(pump(pipe, tx, false));
    }
    drop(tx);

    let mut exit: Option<std::process::ExitStatus> = None;
    let mut timed_out = false;
    let mut stopped = false;
    let mut terminated = false;
    let mut drain_deadline: Option<tokio::time::Instant> = None;

    loop {
        let deadline = input.deadline;
        tokio::select! {
            biased;
            chunk = rx.recv() => {
                let Some((is_stdout, bytes)) = chunk else { break };
                if is_stdout {
                    if plan.parser.is_some() && parser_error.is_none() {
                        feed_parser(&mut splitter, plan.parser.as_mut(), &bytes, &mut parser_error);
                    }
                    stdout_log.push(&bytes);
                    stdout_tail.push(&bytes);
                } else {
                    stderr_log.push(&bytes);
                    stderr_tail.push(&bytes);
                }
            }
            status = child.wait(), if exit.is_none() => {
                exit = status.ok();
            }
            () = wait_until(deadline), if !terminated => {
                terminated = true;
                timed_out = true;
                exit = crate::spawn::signal::terminate_on_timeout(&mut child).await.ok();
                drain_deadline = Some(tokio::time::Instant::now() + crate::spawn::signal::TIMEOUT_SIGTERM_GRACE);
            }
            () = input.stop.cancelled(), if !terminated => {
                terminated = true;
                stopped = true;
                exit = crate::spawn::signal::terminate_on_timeout(&mut child).await.ok();
                drain_deadline = Some(tokio::time::Instant::now() + crate::spawn::signal::TIMEOUT_SIGTERM_GRACE);
            }
            () = wait_until(drain_deadline), if drain_deadline.is_some() => {
                // A descendant that escaped the group can still hold the pipes open; this arm is
                // what makes sure it can never be the thing that hangs the orchestrator.
                break;
            }
        }
    }

    if exit.is_none() {
        exit = child.wait().await.ok();
    }

    // `stdout.once("end")` (`:367-376`): flush the trailing line, then settle the parser.
    let mut parser_terminal: Option<ParserTerminal> = None;
    if let Some(parser) = plan.parser.as_mut()
        && parser_error.is_none()
    {
        for event in splitter.finish() {
            apply_line_event(parser, event, &mut parser_error);
        }
        if parser_error.is_none() {
            parser_terminal = parser.finish();
            parser_error = match &parser_terminal {
                None => Some("External CLI parser did not produce a terminal state.".to_string()),
                Some(terminal)
                    if terminal
                        .output
                        .as_ref()
                        .is_some_and(|output| output.len() > plan.limits.parser_output_bytes) =>
                {
                    Some("External CLI parser terminal output exceeded its byte limit.".to_string())
                }
                Some(terminal)
                    if terminal
                        .error
                        .as_ref()
                        .is_some_and(|error| error.len() > MAX_PARSER_ERROR_BYTES) =>
                {
                    Some("External CLI parser terminal error exceeded its byte limit.".to_string())
                }
                Some(_) => None,
            };
        }
    }

    let ended_at = crate::time::now_epoch_millis();
    let exit_code = exit.as_ref().and_then(std::process::ExitStatus::code);
    let process_signal = exit.as_ref().and_then(signal_name);
    let stderr_text = stderr_tail.text().trim().to_string();

    // `:400-405` — the failure precedence, exactly: stop, then timeout, then a parser failure, then
    // a non-zero exit explained by stderr.
    let parser_failure = parser_error.clone().or_else(|| {
        parser_terminal.as_ref().and_then(|terminal| {
            (!terminal.completed).then(|| {
                terminal
                    .error
                    .clone()
                    .unwrap_or_else(|| "External CLI parser reported terminal failure.".to_string())
            })
        })
    });
    let error = if stopped {
        Some(input.stop_message.clone())
    } else if timed_out {
        Some(input.timeout_message.clone())
    } else if let Some(failure) = parser_failure.clone() {
        Some(failure)
    } else if exit_code == Some(0) {
        None
    } else if stderr_text.is_empty() {
        Some(format!(
            "External CLI exited with code {}.",
            exit_code.map_or_else(|| "null".to_string(), |code| code.to_string())
        ))
    } else {
        Some(stderr_text)
    };

    // `:408` — the parser's terminal output is delivered ONLY when it completed AND no parser error
    // was raised; otherwise the raw stdout tail is what the caller sees.
    let output = match (&parser_error, &parser_terminal) {
        (None, Some(terminal)) if terminal.completed => terminal.output.clone().unwrap_or_default(),
        _ => stdout_tail.text(),
    };

    ExternalCliRunOutcome {
        output: output.trim().to_string(),
        // `:409` — a timeout, a stop or a parser failure is exit 1 regardless of what the process
        // itself reported.
        exit_code: if timed_out || stopped || parser_failure.is_some() {
            1
        } else {
            exit_code.unwrap_or(1)
        },
        error,
        timed_out,
        stopped,
        process_signal: process_signal.clone(),
        external_process: ExternalProcessStatus {
            pid,
            started_at,
            ended_at: Some(ended_at),
            duration_ms: Some(ended_at - started_at),
            exit_code,
            process_signal,
            stdout_path: stdout_path.display().to_string(),
            stderr_path: stderr_path.display().to_string(),
            final_output_path: plan
                .final_output_path
                .as_ref()
                .map(|path| path.display().to_string()),
            stdout_bytes: Some(stdout_log.total()),
            stderr_bytes: Some(stderr_log.total()),
            stdout_truncated: stdout_log.truncated(),
            stderr_truncated: stderr_log.truncated(),
        },
    }
}

/// `parseChunk` (`:314-329`) plus the per-line dispatch at `:273-288`.
fn feed_parser(
    splitter: &mut LineSplitter,
    parser: Option<&mut AdapterParser>,
    bytes: &[u8],
    parser_error: &mut Option<String>,
) {
    let Some(parser) = parser else { return };
    for event in splitter.push(bytes) {
        apply_line_event(parser, event, parser_error);
        if parser_error.is_some() {
            return;
        }
    }
}

fn apply_line_event(
    parser: &mut AdapterParser,
    event: LineEvent,
    parser_error: &mut Option<String>,
) {
    if parser_error.is_some() {
        return;
    }
    match event {
        LineEvent::Line(line) => {
            // A blank line between events is not an event; upstream's `JSON.parse("")` would throw,
            // but its splitter never hands one over because a `\n\n` yields an empty pending line
            // that `parseLine` receives as `""` — which IS a malformed-JSONL failure upstream.
            if let Err(error) = parser.parse_line(&line) {
                *parser_error = Some(error);
            }
        }
        LineEvent::Oversized {
            prefix,
            byte_length,
        } => {
            if parser.skip_oversized_line(&prefix, byte_length).is_none() {
                *parser_error =
                    Some("External CLI parser line exceeded its byte limit.".to_string());
            }
        }
        LineEvent::Failed(error) => *parser_error = Some(error),
    }
}

/// A future that resolves at `deadline`, or never when there is none.
async fn wait_until(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

/// Read a pipe into the shared chunk channel, tagging stdout as `true`.
async fn pump<R: tokio::io::AsyncRead + Unpin + Send + 'static>(
    mut pipe: R,
    tx: tokio::sync::mpsc::Sender<(bool, Vec<u8>)>,
    is_stdout: bool,
) {
    use tokio::io::AsyncReadExt;
    let mut buffer = [0_u8; 8192];
    loop {
        match pipe.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                let chunk = buffer.get(..read).unwrap_or_default().to_vec();
                if tx.send((is_stdout, chunk)).await.is_err() {
                    break;
                }
            }
        }
    }
}

#[cfg(unix)]
fn signal_name(status: &std::process::ExitStatus) -> Option<String> {
    use std::os::unix::process::ExitStatusExt;
    crate::spawn::signal::signal_name_of(status)
        .map(ToString::to_string)
        .or_else(|| status.signal().map(|signal| format!("SIG{signal}")))
}

#[cfg(not(unix))]
fn signal_name(_status: &std::process::ExitStatus) -> Option<String> {
    None
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use crate::exec::external_cli::adapters::{AdapterParser, claude_code::ClaudeCodeParser};
    use crate::exec::external_cli::prompt::PromptDelivery;

    fn script(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("fake-cli.sh");
        std::fs::write(&path, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    fn plan(program: PathBuf, parser: Option<AdapterParser>) -> ExternalCliProcessPlan {
        ExternalCliProcessPlan {
            program,
            args: Vec::new(),
            env: None,
            parser,
            limits: StreamLimits::default(),
            final_output_path: None,
        }
    }

    fn input<'a>(dir: &Path, stop: &'a cyrup_core::CancelToken) -> ExternalCliRunInput<'a> {
        ExternalCliRunInput {
            cwd: dir.to_path_buf(),
            log_dir: dir.join("logs"),
            step_index: 0,
            deadline: None,
            stop,
            timeout_message: "Subagent timed out.".to_string(),
            stop_message: "Subagent stopped by user.".to_string(),
        }
    }

    /// The prompt goes down stdin, the streams are logged and tailed, and the receipt carries the
    /// TRUE byte totals (`external-cli-runner.ts:363-364`, `:393-396`).
    #[tokio::test]
    async fn the_prompt_reaches_stdin_and_both_streams_are_logged() {
        let dir = tempfile::tempdir().unwrap();
        let program = script(dir.path(), "#!/bin/sh\ncat\necho 'diagnostic' >&2\n");
        let prepared = PromptDelivery::Stdin.prepare("hello foreign cli").unwrap();
        let stop = cyrup_core::CancelToken::new();
        let outcome =
            run_external_cli_process(plan(program, None), &prepared, &input(dir.path(), &stop))
                .await;

        assert_eq!(outcome.exit_code, 0, "{outcome:?}");
        assert_eq!(outcome.output, "hello foreign cli");
        assert_eq!(outcome.error, None);
        assert!(!outcome.timed_out && !outcome.stopped);
        let process = &outcome.external_process;
        assert_eq!(process.stdout_bytes, Some(17));
        assert_eq!(process.stderr_bytes, Some(11));
        assert!(!process.stdout_truncated);
        assert_eq!(
            std::fs::read_to_string(&process.stdout_path).unwrap(),
            "hello foreign cli"
        );
        assert_eq!(
            std::fs::read_to_string(&process.stderr_path).unwrap(),
            "diagnostic\n"
        );
    }

    /// A non-zero exit is diagnosed by the process's own stderr (`:405`), and a clean exit with no
    /// stderr gets the code sentence instead.
    #[tokio::test]
    async fn a_non_zero_exit_is_diagnosed_by_stderr_then_by_its_code() {
        let dir = tempfile::tempdir().unwrap();
        let stop = cyrup_core::CancelToken::new();
        let program = script(dir.path(), "#!/bin/sh\necho 'it broke' >&2\nexit 4\n");
        let prepared = PromptDelivery::Stdin.prepare("x").unwrap();
        let outcome =
            run_external_cli_process(plan(program, None), &prepared, &input(dir.path(), &stop))
                .await;
        assert_eq!(outcome.exit_code, 4);
        assert_eq!(outcome.error.as_deref(), Some("it broke"));

        let dir2 = tempfile::tempdir().unwrap();
        let program = script(dir2.path(), "#!/bin/sh\nexit 7\n");
        let prepared = PromptDelivery::Stdin.prepare("x").unwrap();
        let outcome =
            run_external_cli_process(plan(program, None), &prepared, &input(dir2.path(), &stop))
                .await;
        assert_eq!(
            outcome.error.as_deref(),
            Some("External CLI exited with code 7.")
        );
    }

    /// The parser's terminal output is what the caller receives, not the raw stdout tail (`:408`).
    #[tokio::test]
    async fn a_jsonl_parser_terminal_output_replaces_the_raw_stdout_tail() {
        let dir = tempfile::tempdir().unwrap();
        let stop = cyrup_core::CancelToken::new();
        let program = script(
            dir.path(),
            "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{\"type\":\"system\"}' '{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"the answer\"}'\n",
        );
        let prepared = PromptDelivery::Stdin.prepare("x").unwrap();
        let outcome = run_external_cli_process(
            plan(
                program,
                Some(AdapterParser::ClaudeCode(ClaudeCodeParser::new())),
            ),
            &prepared,
            &input(dir.path(), &stop),
        )
        .await;
        assert_eq!(outcome.exit_code, 0, "{outcome:?}");
        assert_eq!(outcome.output, "the answer");
        assert_eq!(outcome.error, None);
    }

    /// A parser protocol failure is exit 1 with the parser's own message, whatever the process
    /// exited with (`:400`, `:409`).
    #[tokio::test]
    async fn a_parser_protocol_failure_fails_the_run_at_exit_one() {
        let dir = tempfile::tempdir().unwrap();
        let stop = cyrup_core::CancelToken::new();
        let program = script(
            dir.path(),
            "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' 'not json'\n",
        );
        let prepared = PromptDelivery::Stdin.prepare("x").unwrap();
        let outcome = run_external_cli_process(
            plan(
                program,
                Some(AdapterParser::ClaudeCode(ClaudeCodeParser::new())),
            ),
            &prepared,
            &input(dir.path(), &stop),
        )
        .await;
        assert_eq!(outcome.exit_code, 1, "{outcome:?}");
        assert!(
            outcome
                .error
                .as_deref()
                .unwrap_or_default()
                .starts_with("Claude Code emitted malformed JSONL:"),
            "{outcome:?}"
        );
    }

    /// A run past its deadline is torn down and reported as a TIMEOUT, and one whose stop token
    /// fires is reported as STOPPED — both at exit 1, with upstream's own messages (`:401-404`).
    /// The teardown targets the child's process GROUP, so a descendant it spawned dies with it.
    #[tokio::test]
    async fn a_deadline_and_a_stop_each_tear_down_the_process_tree() {
        let dir = tempfile::tempdir().unwrap();
        let stop = cyrup_core::CancelToken::new();
        let program = script(dir.path(), "#!/bin/sh\nsleep 60 &\nwait\n");
        let prepared = PromptDelivery::Stdin.prepare("x").unwrap();
        let mut timed = input(dir.path(), &stop);
        timed.deadline = Some(tokio::time::Instant::now() + std::time::Duration::from_millis(150));
        let outcome = run_external_cli_process(plan(program, None), &prepared, &timed).await;
        assert!(outcome.timed_out, "{outcome:?}");
        assert_eq!(outcome.exit_code, 1);
        assert_eq!(outcome.error.as_deref(), Some("Subagent timed out."));

        let dir2 = tempfile::tempdir().unwrap();
        let program = script(dir2.path(), "#!/bin/sh\nsleep 60 &\nwait\n");
        let prepared = PromptDelivery::Stdin.prepare("x").unwrap();
        let stop2 = cyrup_core::CancelToken::new();
        let stopper = stop2.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            stopper.cancel();
        });
        let outcome =
            run_external_cli_process(plan(program, None), &prepared, &input(dir2.path(), &stop2))
                .await;
        assert!(outcome.stopped, "{outcome:?}");
        assert_eq!(outcome.exit_code, 1);
        assert_eq!(outcome.error.as_deref(), Some("Subagent stopped by user."));
    }

    /// A binary that cannot be spawned settles the run at exit 1 WITHOUT a process, and the receipt
    /// still names the log paths (`:211-224`).
    #[tokio::test]
    async fn an_unspawnable_binary_settles_the_run_without_a_process() {
        let dir = tempfile::tempdir().unwrap();
        let stop = cyrup_core::CancelToken::new();
        let prepared = PromptDelivery::Stdin.prepare("x").unwrap();
        let outcome = run_external_cli_process(
            plan(dir.path().join("does-not-exist"), None),
            &prepared,
            &input(dir.path(), &stop),
        )
        .await;
        assert_eq!(outcome.exit_code, 1);
        assert!(outcome.error.is_some());
        assert_eq!(outcome.external_process.pid, None);
        assert!(
            outcome
                .external_process
                .stdout_path
                .ends_with(".stdout.log")
        );
    }

    /// The stream log stops at its cap while the receipt still reports the TRUE total and flags the
    /// truncation (`:126-136`, `:393-396`).
    #[tokio::test]
    async fn an_oversized_stream_is_logged_up_to_its_cap_and_flagged() {
        let dir = tempfile::tempdir().unwrap();
        let stop = cyrup_core::CancelToken::new();
        let program = script(
            dir.path(),
            "#!/bin/sh\ncat >/dev/null\nfor i in 1 2 3 4 5 6 7 8; do printf 'aaaaaaaaaa'; done\n",
        );
        let prepared = PromptDelivery::Stdin.prepare("x").unwrap();
        let mut process_plan = plan(program, None);
        process_plan.limits =
            StreamLimits::narrowed(Some(16), None, None, None, None).expect("narrowed");
        let outcome =
            run_external_cli_process(process_plan, &prepared, &input(dir.path(), &stop)).await;
        assert_eq!(outcome.external_process.stdout_bytes, Some(80));
        assert!(outcome.external_process.stdout_truncated);
        assert_eq!(
            std::fs::read(&outcome.external_process.stdout_path)
                .unwrap()
                .len(),
            16
        );
        assert_eq!(
            outcome.output.len(),
            80,
            "the delivered output comes from the TAIL, which is bounded separately"
        );
    }
}
