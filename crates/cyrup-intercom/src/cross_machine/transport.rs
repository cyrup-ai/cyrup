//! `cross-machine-transport.ts` (`v0.16.0`) — the process runner and the SSH send.
//!
//! The runner is upstream's own rather than [`cyrup_herdr::HerdrCli`]: that one nulls stdin,
//! interprets herdr's response envelope and reports [`cyrup_herdr::CliError`], while this one must
//! WRITE stdin (the relay envelope), hand back raw `stdout`/`stderr`/`code`, and run `ssh` as
//! readily as `herdr`. Upstream keeps the same two runners apart for the same reason
//! (`project-agent.ts`'s `run<T>` vs this file's `runCommand`).

use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::io::AsyncWriteExt;

use super::discovery::{
    DISCOVERY_TIMEOUT, DiscoveredRemoteAgent, DiscoveryDeps, DiscoveryError, discover_remote_agent,
};
use super::envelope::{CrossMachineEnvelope, CrossMachineOrigin};

/// `process.env.HERDR_BIN_PATH` (`v0.16.0 cross-machine-transport.ts:79`, `index.ts:3052`).
///
/// **Not `HERDR_BIN`,** and not a typo here: upstream's cross-machine files read this *other*
/// spelling, while `project-agent.ts:68` (cyrup: [`cyrup_herdr::cli::HERDR_BIN`]) reads
/// `HERDR_BIN`, and neither falls back to the other. Adding a fallback would be inventing upstream
/// behaviour, so the two variables stay as upstream has them; both belong to the herdr vendor, so
/// neither takes a `CYRUP_` prefix.
pub const HERDR_BIN_PATH: &str = "HERDR_BIN_PATH";

/// The `herdr` binary cross-machine discovery runs: `deps.herdrBin ?? process.env.HERDR_BIN_PATH ??
/// "herdr"` (`v0.16.0 cross-machine-transport.ts:79`), the environment half.
///
/// A blank value falls through to `"herdr"`, as every other binary ladder in this workspace does
/// for a variable that was unset badly.
#[must_use]
pub fn herdr_bin_from(env: impl Fn(&str) -> Option<String>) -> String {
    env(HERDR_BIN_PATH)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| cyrup_herdr::cli::HERDR_BIN_DEFAULT.to_string())
}

/// `DELIVERY_TIMEOUT_MS = 15_000` (`v0.16.0 cross-machine-transport.ts:5`).
pub const DELIVERY_TIMEOUT: Duration = Duration::from_millis(15_000);

/// `CommandResult` (`v0.16.0 cross-machine-transport.ts:7-12`).
///
/// `timed_out` is a plain `bool` where upstream spreads `...(timedOut ? { timedOut: true } : {})`:
/// the absent key and `false` are the same thing to every reader (`listed.timedOut ? …`), and the
/// spread exists only because JS has no way to say "absent".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandResult {
    /// Everything the child wrote to stdout.
    pub stdout: String,
    /// Everything the child wrote to stderr.
    pub stderr: String,
    /// `timedOut ? 124 : (code ?? 1)` (`:54`) — 124 is the shell convention for a killed timeout,
    /// and a signal-terminated child reports 1 rather than `null`.
    pub code: i32,
    /// Whether the deadline, not the child, ended it.
    pub timed_out: bool,
}

/// `CommandRunner` (`v0.16.0 cross-machine-transport.ts:14`) — the seam both this module and
/// [`super::discovery`] take their processes through.
#[async_trait::async_trait]
pub trait CommandRunner: Send + Sync {
    /// Run `command args`, write `stdin` (upstream always ends the pipe, even with no input), and
    /// collect the result under `timeout`.
    ///
    /// # Errors
    /// The process could not be started or its output could not be collected — upstream's REJECTED
    /// promise (`onError` → `reject`), as distinct from a non-zero `code`.
    async fn run(
        &self,
        command: &str,
        args: &[&str],
        stdin: Option<&str>,
        timeout: Option<Duration>,
    ) -> std::io::Result<CommandResult>;
}

/// `runCommand` (`v0.16.0 cross-machine-transport.ts:34-66`, whose process contract `53580c2`
/// hardened).
///
/// The three properties that commit named: the promise settles exactly ONCE, the timeout
/// `SIGKILL`s rather than `SIGTERM`s, and a timed-out run reports code 124 with the flag set.
/// Single-settle is structural here — one `timeout` decides — and the kill is explicit.
///
/// A timed-out run still hands back **what the child had written** (`stdout`/`stderr` are
/// accumulated by `data` handlers upstream and returned from `onClose` whether or not the timer
/// fired). That is observable: a relay that printed `{"ok":false,"error":…}` and then hung is
/// reported as `Remote intercom delivery … failed: <error>` rather than as a missing relay.
#[derive(Clone, Copy, Debug, Default)]
pub struct SpawnRunner;

/// How long a killed child, and the readers draining its pipes, get to finish after the deadline.
///
/// Upstream waits for the `close` event, which a grandchild holding the pipe open can postpone
/// forever; here the wait is bounded so a timed-out run still returns promptly with what was read.
const KILL_REAP_GRACE: Duration = Duration::from_secs(1);

/// The two tasks copying a child's stdout and stderr into their sinks.
struct Readers {
    stdout: Option<tokio::task::JoinHandle<()>>,
    stderr: Option<tokio::task::JoinHandle<()>>,
}

impl Readers {
    /// Wait for both pipes to reach EOF.
    async fn finished(&mut self) {
        if let Some(task) = self.stdout.as_mut() {
            let _ = task.await;
        }
        if let Some(task) = self.stderr.as_mut() {
            let _ = task.await;
        }
    }

    fn abort(&self) {
        for task in [&self.stdout, &self.stderr].into_iter().flatten() {
            task.abort();
        }
    }
}

/// `child.on("close")`: the process has exited AND its stdio has closed.
async fn wait_and_drain(
    child: &mut tokio::process::Child,
    readers: &mut Readers,
) -> std::io::Result<std::process::ExitStatus> {
    let status = child.wait().await?;
    readers.finished().await;
    Ok(status)
}

/// Copy `pipe` into `sink` as it arrives, so a reader that is abandoned at the deadline has
/// already published everything read so far.
async fn drain<R: tokio::io::AsyncRead + Unpin>(mut pipe: R, sink: Arc<Mutex<Vec<u8>>>) {
    use tokio::io::AsyncReadExt;
    let mut chunk = [0u8; 8192];
    loop {
        match pipe.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(read) => {
                if let Some(bytes) = chunk.get(..read) {
                    sink.lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .extend_from_slice(bytes);
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl CommandRunner for SpawnRunner {
    async fn run(
        &self,
        command: &str,
        args: &[&str],
        stdin: Option<&str>,
        timeout: Option<Duration>,
    ) -> std::io::Result<CommandResult> {
        let mut child = tokio::process::Command::new(command)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let stdout = Arc::new(Mutex::new(Vec::new()));
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let mut readers = Readers {
            stdout: child
                .stdout
                .take()
                .map(|pipe| tokio::spawn(drain(pipe, Arc::clone(&stdout)))),
            stderr: child
                .stderr
                .take()
                .map(|pipe| tokio::spawn(drain(pipe, Arc::clone(&stderr)))),
        };
        // `child.stdin.end(input)` (`:65`) — ALWAYS closed, input or not. A remote relay reading
        // `--envelope-stdin` blocks forever on a pipe that is merely empty.
        //
        // The write runs INSIDE the deadline. Node's `end(input)` only queues the bytes, so a
        // child that never reads stdin cannot stall the caller there; a blocking `write_all` of an
        // envelope larger than the pipe buffer (64 KiB, against a 256 KiB text cap) would, and
        // would sit outside the 15 s delivery timeout.
        let mut stdin_pipe = child.stdin.take();
        let run_to_close = async {
            if let Some(mut pipe) = stdin_pipe.take() {
                if let Some(input) = stdin {
                    // A write failure is not fatal: the child may have exited already, which the
                    // exit code below reports better than an io error would.
                    let _ = pipe.write_all(input.as_bytes()).await;
                }
                let _ = pipe.shutdown().await;
            }
            wait_and_drain(&mut child, &mut readers).await
        };
        let finished = match timeout {
            None => Some(run_to_close.await),
            Some(limit) => tokio::time::timeout(limit, run_to_close).await.ok(),
        };
        let text = |sink: &Mutex<Vec<u8>>| {
            String::from_utf8_lossy(&sink.lock().unwrap_or_else(PoisonError::into_inner))
                .into_owned()
        };
        if let Some(status) = finished {
            return Ok(CommandResult {
                stdout: text(&stdout),
                stderr: text(&stderr),
                // `code ?? 1` — a signal-killed child has no code and reports 1.
                code: status?.code().unwrap_or(1),
                timed_out: false,
            });
        }
        // `child.kill("SIGKILL")` (`:59`), then reap it and let the readers drain whatever the
        // pipes still hold.
        let _ = child.start_kill();
        let _ =
            tokio::time::timeout(KILL_REAP_GRACE, wait_and_drain(&mut child, &mut readers)).await;
        readers.abort();
        Ok(CommandResult {
            stdout: text(&stdout),
            stderr: text(&stderr),
            code: 124,
            timed_out: true,
        })
    }
}

/// What a cross-machine send needs beyond the target, text and origin
/// (`CrossMachineDeps`, `v0.16.0 cross-machine-transport.ts:16-22`).
pub struct CrossMachineDeps<'a> {
    /// The process runner.
    pub run: &'a dyn CommandRunner,
    /// `deps.herdrBin ?? process.env.HERDR_BIN_PATH ?? "herdr"` (`:79`).
    pub herdr_bin: &'a str,
    /// `config.crossMachine.remoteCommand`.
    pub remote_command: &'a str,
    /// Discovery deadline (`DISCOVERY_TIMEOUT` by default).
    pub discovery_timeout: Duration,
    /// Delivery deadline (`DELIVERY_TIMEOUT` by default).
    pub delivery_timeout: Duration,
}

impl<'a> CrossMachineDeps<'a> {
    /// Production defaults around a runner and the two config-driven strings.
    #[must_use]
    pub fn new(run: &'a dyn CommandRunner, herdr_bin: &'a str, remote_command: &'a str) -> Self {
        Self {
            run,
            herdr_bin,
            remote_command,
            discovery_timeout: DISCOVERY_TIMEOUT,
            delivery_timeout: DELIVERY_TIMEOUT,
        }
    }
}

/// `CrossMachineDelivery` (`v0.16.0 cross-machine-transport.ts:24-28`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrossMachineDelivery {
    /// The machine and agent the envelope was addressed to.
    pub discovered: DiscoveredRemoteAgent,
    /// The relay's raw stdout — the JSON `{ok, delivered, id, origin, trust}` object.
    pub stdout: String,
}

/// Why a cross-machine send did not land.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CrossMachineError {
    /// `"Remote command must not be empty."` (`:81`).
    #[error("Remote command must not be empty.")]
    EmptyRemoteCommand,
    /// `"Remote command must not contain ASCII control characters."` (`:83`) — "SSH interprets its
    /// remote command string, so control characters are not safe here".
    #[error("Remote command must not contain ASCII control characters.")]
    ControlCharactersInRemoteCommand,
    /// `name@machine` did not resolve.
    #[error("{0}")]
    Discovery(#[from] DiscoveryError),
    /// `ssh` itself could not be started.
    #[error("{message}")]
    SshFailed {
        /// The io error's text.
        message: String,
    },
    /// `relaySupportError(machine)` (`:68-70`) — the reply was not a v1 relay response at all, so
    /// the remote end is not a relay-capable intercom. Deliberately NOT a delivery failure: the
    /// sentence tells the operator to upgrade, not to retry.
    #[error(
        "Remote cyrup-intercom on \"{machine}\" has no compatible relay support and needs upgrading."
    )]
    NoRelaySupport {
        /// The saved machine's label.
        machine: String,
    },
    /// The remote shell could not find `remoteCommand` at all: no JSON on stdout and exit status
    /// 127, POSIX `sh`'s "command not found" (ssh passes the remote status through; its own
    /// failures are 255).
    ///
    /// [CYRUP-DELTA] upstream folds this into `relaySupportError` (`:100-104`), which tells the
    /// operator to UPGRADE a binary that is not there. The usual cause is not a missing install but
    /// a non-interactive ssh `PATH` that lacks the directory `cyrup` lives in (`~/.cargo/bin` is
    /// added by login profiles that `ssh host cmd` never reads), and the fix is a different one, so
    /// it gets its own sentence. Any other unreadable reply is still [`Self::NoRelaySupport`].
    #[error(
        "Remote command \"{command}\" was not found on \"{machine}\". Install cyrup there, or set \
         crossMachine.remoteCommand to its absolute path (non-interactive ssh often lacks \
         ~/.cargo/bin on PATH)."
    )]
    RemoteCommandNotFound {
        /// The saved machine's label.
        machine: String,
        /// The first word of `crossMachine.remoteCommand` — the program the shell looked up.
        command: String,
    },
    /// The relay answered `{ok: false, error}` or exited non-zero with a readable error (`:109`).
    #[error("Remote intercom delivery via {machine} failed: {error}")]
    RemoteRefused {
        /// The saved machine's label.
        machine: String,
        /// The relay's own `error` string.
        error: String,
    },
}

/// `sendCrossMachine(target, text, origin, deps)` (`v0.16.0 cross-machine-transport.ts:72-110`).
///
/// Order matters and is upstream's: the `remoteCommand` is validated BEFORE discovery, so a
/// malformed config costs no `herdr` round trips; discovery resolves `name@machine` to exactly one
/// agent; then `ssh <machine.target> "<remoteCommand> relay --envelope-stdin --json"` carries the
/// envelope on stdin.
///
/// The envelope's `target` is `agent.sessionId ?? agent.name` (`:89`) — the session id when
/// discovery recovered one, because a name is unique only among RENAMED panes.
///
/// # Errors
/// Any [`CrossMachineError`].
pub async fn send_cross_machine(
    target: &str,
    text: &str,
    origin: CrossMachineOrigin,
    deps: &CrossMachineDeps<'_>,
) -> Result<CrossMachineDelivery, CrossMachineError> {
    if deps.remote_command.trim().is_empty() {
        return Err(CrossMachineError::EmptyRemoteCommand);
    }
    // `/[\x00-\x1f\x7f]/` (`:83`).
    if deps.remote_command.bytes().any(|b| b < 0x20 || b == 0x7f) {
        return Err(CrossMachineError::ControlCharactersInRemoteCommand);
    }
    let discovered = discover_remote_agent(
        target,
        &DiscoveryDeps {
            run: deps.run,
            herdr_bin: deps.herdr_bin,
            discovery_timeout: deps.discovery_timeout,
        },
    )
    .await?;
    let envelope = CrossMachineEnvelope::new(
        discovered
            .agent
            .session_id
            .clone()
            .unwrap_or_else(|| discovered.agent.name.clone()),
        text,
        origin,
    );
    // `JSON.stringify(envelope)` cannot fail for this struct — every field is a `String`, a `u8` or
    // a unit-variant enum — so the fallback is unreachable rather than a silent empty send.
    let payload = format!("{}\n", serde_json::to_string(&envelope).unwrap_or_default());
    let remote_argv = format!("{} relay --envelope-stdin --json", deps.remote_command);
    let delivered = deps
        .run
        .run(
            "ssh",
            &[discovered.machine.target.as_str(), remote_argv.as_str()],
            Some(&payload),
            Some(deps.delivery_timeout),
        )
        .await
        .map_err(|error| CrossMachineError::SshFailed {
            message: error.to_string(),
        })?;
    let no_support = || CrossMachineError::NoRelaySupport {
        machine: discovered.machine.label.clone(),
    };
    // POSIX `sh`: 127 = "command not found" — and a missing program prints nothing on stdout.
    const SHELL_COMMAND_NOT_FOUND: i32 = 127;
    let response: serde_json::Value = match serde_json::from_str(&delivered.stdout) {
        Ok(response) => response,
        Err(_)
            if delivered.code == SHELL_COMMAND_NOT_FOUND && delivered.stdout.trim().is_empty() =>
        {
            return Err(CrossMachineError::RemoteCommandNotFound {
                machine: discovered.machine.label.clone(),
                command: deps
                    .remote_command
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_string(),
            });
        }
        Err(_) => return Err(no_support()),
    };
    // `!isRecord(response) || typeof response.ok !== "boolean" || ("version" in response &&
    // response.version !== 1)` (`:104`) — a `version` key that is present and not 1 is an
    // incompatible relay, while an ABSENT one is the v0.16.0 shape and fine.
    let object = response.as_object().ok_or_else(no_support)?;
    let ok = object
        .get("ok")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(no_support)?;
    // A number comparison, so `1.0` is as good as `1`.
    if let Some(version) = object.get("version")
        && version.as_f64() != Some(1.0)
    {
        return Err(no_support());
    }
    if delivered.code != 0 || !ok {
        let error = object
            .get("error")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(no_support)?;
        return Err(CrossMachineError::RemoteRefused {
            machine: discovered.machine.label.clone(),
            error: error.to_string(),
        });
    }
    Ok(CrossMachineDelivery {
        discovered,
        stdout: delivered.stdout,
    })
}
