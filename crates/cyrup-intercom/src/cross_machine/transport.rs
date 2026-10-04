//! `cross-machine-transport.ts` (`v0.16.0`) — the process runner and the SSH send.
//!
//! The runner is upstream's own rather than [`cyrup_herdr::HerdrCli`]: that one nulls stdin,
//! interprets herdr's response envelope and reports [`cyrup_herdr::CliError`], while this one must
//! WRITE stdin (the relay envelope), hand back raw `stdout`/`stderr`/`code`, and run `ssh` as
//! readily as `herdr`. Upstream keeps the same two runners apart for the same reason
//! (`project-agent.ts`'s `run<T>` vs this file's `runCommand`).

use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncWriteExt;

use super::discovery::{
    DISCOVERY_TIMEOUT, DiscoveredRemoteAgent, DiscoveryDeps, DiscoveryError, discover_remote_agent,
};
use super::envelope::{CrossMachineEnvelope, CrossMachineOrigin};

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
/// Single-settle is structural here — `tokio::select!` takes one arm — and `kill_on_drop` is the
/// `SIGKILL`: dropping the child on the timeout arm kills it, where `wait_with_output` would
/// otherwise wait for a child that is ignoring signals.
#[derive(Clone, Copy, Debug, Default)]
pub struct SpawnRunner;

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
        // `child.stdin.end(input)` (`:65`) — ALWAYS closed, input or not. A remote relay reading
        // `--envelope-stdin` blocks forever on a pipe that is merely empty.
        if let Some(mut pipe) = child.stdin.take() {
            if let Some(input) = stdin {
                // A write failure is not fatal: the child may have exited already, which the
                // exit code below reports better than an io error would.
                let _ = pipe.write_all(input.as_bytes()).await;
            }
            let _ = pipe.shutdown().await;
        }
        let collected = match timeout {
            None => child.wait_with_output().await?,
            Some(limit) => {
                tokio::select! {
                    output = child.wait_with_output() => output?,
                    () = tokio::time::sleep(limit) => {
                        return Ok(CommandResult {
                            stdout: String::new(),
                            stderr: String::new(),
                            code: 124,
                            timed_out: true,
                        });
                    }
                }
            }
        };
        Ok(CommandResult {
            stdout: String::from_utf8_lossy(&collected.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&collected.stderr).into_owned(),
            // `code ?? 1` — a signal-killed child has no code and reports 1.
            code: collected.status.code().unwrap_or(1),
            timed_out: false,
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
    let response: serde_json::Value =
        serde_json::from_str(&delivered.stdout).map_err(|_| no_support())?;
    // `!isRecord(response) || typeof response.ok !== "boolean" || ("version" in response &&
    // response.version !== 1)` (`:104`) — a `version` key that is present and not 1 is an
    // incompatible relay, while an ABSENT one is the v0.16.0 shape and fine.
    let object = response.as_object().ok_or_else(no_support)?;
    let ok = object
        .get("ok")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(no_support)?;
    if let Some(version) = object.get("version")
        && version != &serde_json::json!(1)
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
