//! Reaching a herdr that runs on a **saved SSH machine** — bounded ssh commands, remote endpoint
//! discovery, and OpenSSH StreamLocal forwarding of the remote herdr socket to a local one.
//!
//! Ported from pi-subagents' `src/runs/shared/herdr-connection.ts` @v0.68.0, which is the whole of
//! how pi reaches a remote herdr. The design rule is upstream's and it is load-bearing: **ssh is
//! bounded transport and never owns an agent.** Every command here either finishes under a
//! deadline ([`SshTransport::run`]) or is a pure `-N` forward ([`MachineConnection`]); nothing
//! placed on a machine runs as a child of an ssh process. The agent lives in a herdr-owned pane on
//! that machine, and losing ssh loses only the view of it.
//!
//! Once the socket is forwarded, the remote herdr is an ordinary [`crate::HerdrClient`] over a
//! local path — the same client, the same one-request-per-connection framing, the same bounds —
//! so this module adds a transport, not a second herdr client.
//!
//! ## Hardening, as upstream has it
//!
//! * the ssh child gets a scrubbed environment: `PATH=/usr/bin:/bin:/usr/sbin:/sbin` and nothing
//!   else (`hardenedSshEnv`, `:16`), plus `SendEnv=-*` so ssh forwards none of it either;
//! * `BatchMode=yes`, `StrictHostKeyChecking=yes`, no password prompts, `ForwardAgent=no`, bounded
//!   connect and keepalive ([`HERDR_SSH_BASE`], `:15`);
//! * an agent socket is passed as `IdentityAgent=` rather than through the environment (`:17`);
//! * every remote command runs under a fixed `PATH` ([`HERDR_REMOTE_PATH`], `:14`), because a
//!   non-interactive ssh shell skips the rc files that would otherwise set one.
//!
//! **`[CYRUP-DELTA]` — the ssh binary.** Upstream spawns `options.sshBin ?? "ssh"` resolved
//! through that hardened `PATH`, with `sshBin` a test-only option. cyrup names the binary through
//! [`SSH_BIN_ENV`] (default `ssh`), which is both the test seam and the knob an operator with an
//! OpenSSH outside `/usr/bin` (Homebrew's, say) needs; a bare name is still resolved through the
//! hardened `PATH` only.
//!
//! Unix-only, like upstream: `connectHerdrMachine` refuses Windows outright (`:120`), because
//! StreamLocal forwarding is the transport.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::client::HerdrClient;
use crate::env::EnvSource;

/// `HERDR_REMOTE_PATH` (`herdr-connection.ts:14`) — the `PATH` every remote command runs under.
pub const HERDR_REMOTE_PATH: &str =
    "$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";

/// The `PATH` the local ssh child gets (`hardenedSshEnv`, `herdr-connection.ts:16`).
pub const HARDENED_SSH_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// `HERDR_SSH_BASE` (`herdr-connection.ts:15`), verbatim — except the guard variable is cyrup's.
pub const HERDR_SSH_BASE: [&str; 23] = [
    "-T",
    "-o",
    "BatchMode=yes",
    "-o",
    "NumberOfPasswordPrompts=0",
    "-o",
    "StrictHostKeyChecking=yes",
    "-o",
    "ConnectTimeout=10",
    "-o",
    "ConnectionAttempts=1",
    "-o",
    "ForwardAgent=no",
    "-o",
    "ExitOnForwardFailure=yes",
    "-o",
    "ServerAliveInterval=15",
    "-o",
    "ServerAliveCountMax=4",
    "-o",
    "SendEnv=-*",
    "-o",
    "SetEnv=CYRUP_SUBAGENTS_SSH_GUARD=",
];

/// The environment variable naming the local ssh binary (module doc, `[CYRUP-DELTA]`).
pub const SSH_BIN_ENV: &str = "CYRUP_HERDR_SSH_BIN";

/// `MAX_DISCOVERY_BYTES` (`herdr-connection.ts:8`).
pub const MAX_DISCOVERY_BYTES: usize = 256 * 1024;

/// The default deadline of one bounded remote command (`runHerdrRemoteCommandAsync`, `:36`).
pub const REMOTE_COMMAND_TIMEOUT: Duration = Duration::from_secs(15);

/// A failure reaching the remote herdr. The message is pi's sentence where pi has one.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct RemoteError(pub String);

/// `shellQuoteRemote(value)` (`herdr-connection.ts:25`): always single-quote, POSIX-escaping every
/// embedded quote. Unlike [`crate::cli::shell_quote`] this never takes a Windows arm — the remote
/// is a POSIX shell by contract.
#[must_use]
pub fn shell_quote_remote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// `remoteShellCommand(script, args)` (`herdr-connection.ts:26`): `sh -c '<PATH=…; script>' sh
/// '<arg>'…`, so the script sees its arguments as `$1…` and never has them spliced into its text.
#[must_use]
pub fn remote_shell_command(script: &str, args: &[&str]) -> String {
    let body = format!("PATH=\"{HERDR_REMOTE_PATH}\"; export PATH; {script}");
    let mut command = format!("sh -c {} sh", shell_quote_remote(&body));
    for arg in args {
        command.push(' ');
        command.push_str(&shell_quote_remote(arg));
    }
    command
}

/// `canonicalHerdrSession` (`herdr-connection.ts:27`): `"default"` and blank are the default
/// session, which herdr names `None`.
#[must_use]
pub fn canonical_session(session: Option<&str>) -> Option<String> {
    session
        .filter(|value| !value.is_empty() && *value != "default")
        .map(str::to_string)
}

/// `sshEnvCommand(session, command)` (`herdr-connection.ts:28`): run `command` with any inherited
/// herdr socket/session cleared and the saved machine's own session selected.
#[must_use]
pub fn ssh_env_command(session: Option<&str>, command: &str) -> String {
    let assignment = canonical_session(session)
        .map(|selected| format!(" HERDR_SESSION={}", shell_quote_remote(&selected)))
        .unwrap_or_default();
    format!(
        "/usr/bin/env -u HERDR_SOCKET_PATH -u HERDR_SESSION{assignment} {}",
        remote_shell_command(command, &[])
    )
}

/// What one bounded remote command produced (`runHerdrRemoteCommandAsync`'s resolve value).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemoteOutput {
    /// The exit code; `None` when a signal ended it or it never started.
    pub status: Option<i32>,
    /// stdout, lossily decoded.
    pub stdout: String,
    /// stderr, lossily decoded.
    pub stderr: String,
    /// Why the command did not complete normally (spawn failure, deadline, byte bound).
    pub error: Option<String>,
}

/// The remote herdr endpoint `herdr status server --json` reported (`HerdrEndpoint`, `:11`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEndpoint {
    /// The remote API socket — an absolute POSIX path on the machine.
    pub socket: String,
    /// The canonical session (`None` = default).
    pub session: Option<String>,
    /// herdr's version string.
    pub version: String,
    /// herdr's protocol number.
    pub protocol: u64,
}

/// `parseHerdrEndpoint(value, expectedSession)` (`herdr-connection.ts:49-60`), against the JSON
/// `herdr status server --json` prints (`tmp/herdr/src/cli/status.rs:262-275`).
///
/// # Errors
/// pi's sentence for each refusal.
pub fn parse_endpoint(
    value: &str,
    expected_session: Option<&str>,
) -> Result<RemoteEndpoint, RemoteError> {
    let parsed: serde_json::Value = serde_json::from_str(value).map_err(|_| {
        RemoteError("Remote Herdr endpoint discovery returned malformed JSON.".to_string())
    })?;
    let Some(p) = parsed.as_object() else {
        return Err(RemoteError(
            "Remote Herdr endpoint discovery returned no endpoint.".to_string(),
        ));
    };
    let socket = p.get("socket").and_then(serde_json::Value::as_str);
    let version = p.get("version").and_then(serde_json::Value::as_str);
    let protocol = p.get("protocol").and_then(serde_json::Value::as_f64);
    let (Some(socket), Some(version), Some(protocol)) = (socket, version, protocol) else {
        return Err(RemoteError(
            "Remote Herdr endpoint discovery returned incomplete identity.".to_string(),
        ));
    };
    if !socket.starts_with('/') {
        return Err(RemoteError(
            "Remote Herdr endpoint discovery returned incomplete identity.".to_string(),
        ));
    }
    let session = canonical_session(p.get("session").and_then(serde_json::Value::as_str));
    let expected = canonical_session(expected_session);
    if expected != session {
        return Err(RemoteError(format!(
            "Remote Herdr session identity mismatch: expected {}, received {}.",
            expected.as_deref().unwrap_or("default"),
            session.as_deref().unwrap_or("default")
        )));
    }
    let running = p.get("running") == Some(&serde_json::Value::Bool(true));
    let compatible = p.get("compatible") == Some(&serde_json::Value::Bool(true));
    let endpoint_incompatible =
        p.get("endpoint_compatible") == Some(&serde_json::Value::Bool(false));
    if !running || !compatible || endpoint_incompatible {
        return Err(RemoteError(
            "The selected remote Herdr session is stopped or incompatible.".to_string(),
        ));
    }
    // `protocol` is a JSON number upstream (`typeof p.protocol !== "number"`); herdr writes a u32.
    let protocol = if protocol.is_finite() && protocol >= 0.0 {
        // Truncation is the intent: herdr never writes a fraction here.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let whole = protocol as u64;
        whole
    } else {
        return Err(RemoteError(
            "Remote Herdr endpoint discovery returned incomplete identity.".to_string(),
        ));
    };
    Ok(RemoteEndpoint {
        socket: socket.to_string(),
        session,
        version: version.to_string(),
        protocol,
    })
}

/// The ssh side of one saved machine: which binary, and which agent socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshTransport {
    bin: String,
    auth_sock: Option<String>,
}

impl SshTransport {
    /// A transport through `bin`, passing `auth_sock` as `IdentityAgent=` when set.
    #[must_use]
    pub fn new(bin: impl Into<String>, auth_sock: Option<String>) -> Self {
        Self {
            bin: bin.into(),
            auth_sock: auth_sock.filter(|value| !value.is_empty()),
        }
    }

    /// [`SSH_BIN_ENV`] (trimmed, non-blank) else `ssh`, and `SSH_AUTH_SOCK`.
    #[must_use]
    pub fn with_env(env: &impl EnvSource) -> Self {
        let bin = env
            .var(SSH_BIN_ENV)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "ssh".to_string());
        Self::new(bin, env.var("SSH_AUTH_SOCK"))
    }

    /// The ssh binary.
    #[must_use]
    pub fn bin(&self) -> &str {
        &self.bin
    }

    /// `herdrSshArgs(env)` (`herdr-connection.ts:17`).
    #[must_use]
    pub fn args(&self) -> Vec<String> {
        let mut args: Vec<String> = HERDR_SSH_BASE.iter().map(|s| (*s).to_string()).collect();
        if let Some(sock) = &self.auth_sock {
            args.push("-o".to_string());
            args.push(format!("IdentityAgent={sock}"));
        }
        args
    }

    /// An ssh `Command` with the hardened environment and no argv yet.
    fn command(&self) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(&self.bin);
        command
            .env_clear()
            .env("PATH", HARDENED_SSH_PATH)
            .kill_on_drop(true);
        command
    }

    /// `runHerdrRemoteCommandAsync(machine, command, options)` (`herdr-connection.ts:36-47`): run
    /// `command` on `target` under `timeout`, stdout and stderr each bounded by `max_bytes`, with
    /// `stdin` written to the remote command when given (and `/dev/null` otherwise).
    ///
    /// Never an `Err`: like upstream, every failure is reported in [`RemoteOutput::error`] and the
    /// caller decides what it means.
    pub async fn run(
        &self,
        target: &str,
        command: &str,
        timeout: Duration,
        max_bytes: usize,
        stdin: Option<&[u8]>,
    ) -> RemoteOutput {
        let mut process = self.command();
        process
            .args(self.args())
            .arg(target)
            .arg(command)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = match process.spawn() {
            Ok(child) => child,
            Err(error) => {
                return RemoteOutput {
                    error: Some(error.to_string()),
                    ..RemoteOutput::default()
                };
            }
        };
        let input = stdin.map(<[u8]>::to_vec);
        let mut child_stdin = child.stdin.take();
        let (Some(mut stdout), Some(mut stderr)) = (child.stdout.take(), child.stderr.take())
        else {
            return RemoteOutput {
                error: Some("the ssh child's output pipes were not available".to_string()),
                ..RemoteOutput::default()
            };
        };
        let limit = u64::try_from(max_bytes)
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        let run = async {
            let feed = async {
                if let (Some(pipe), Some(bytes)) = (child_stdin.as_mut(), input.as_ref()) {
                    // A remote that closes its stdin early is its own failure, reported by its
                    // exit status; the write error adds nothing.
                    let _ = pipe.write_all(bytes).await;
                    let _ = pipe.shutdown().await;
                }
                drop(child_stdin.take());
            };
            let mut out = Vec::new();
            let mut err = Vec::new();
            let mut out_pipe = (&mut stdout).take(limit);
            let mut err_pipe = (&mut stderr).take(limit);
            let ((), out_read, err_read) = tokio::join!(
                feed,
                out_pipe.read_to_end(&mut out),
                err_pipe.read_to_end(&mut err),
            );
            out_read?;
            err_read?;
            Ok::<_, std::io::Error>((out, err))
        };
        let (out, err) = match tokio::time::timeout(timeout, run).await {
            Ok(Ok(collected)) => collected,
            Ok(Err(error)) => {
                let _ = child.start_kill();
                return RemoteOutput {
                    error: Some(error.to_string()),
                    ..RemoteOutput::default()
                };
            }
            Err(_) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return RemoteOutput {
                    error: Some(format!(
                        "ssh {target} timed out after {} ms",
                        timeout.as_millis()
                    )),
                    ..RemoteOutput::default()
                };
            }
        };
        let overflow = if out.len() > max_bytes {
            Some("stdout maxBuffer length exceeded")
        } else if err.len() > max_bytes {
            Some("stderr maxBuffer length exceeded")
        } else {
            None
        };
        if let Some(message) = overflow {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return RemoteOutput {
                stdout: String::from_utf8_lossy(&out).into_owned(),
                stderr: String::from_utf8_lossy(&err).into_owned(),
                error: Some(message.to_string()),
                status: None,
            };
        }
        let status = match tokio::time::timeout(timeout, child.wait()).await {
            Ok(Ok(status)) => status.code(),
            Ok(Err(error)) => {
                return RemoteOutput {
                    stdout: String::from_utf8_lossy(&out).into_owned(),
                    stderr: String::from_utf8_lossy(&err).into_owned(),
                    error: Some(error.to_string()),
                    status: None,
                };
            }
            Err(_) => {
                let _ = child.start_kill();
                None
            }
        };
        RemoteOutput {
            status,
            stdout: String::from_utf8_lossy(&out).into_owned(),
            stderr: String::from_utf8_lossy(&err).into_owned(),
            error: None,
        }
    }

    /// A LONG-LIVED remote command whose stdout the caller reads as a stream — the one ssh
    /// process shape [`Self::run`] (bounded, collected) and the `-N` forwards do not cover.
    ///
    /// Same hardened binary, argv and environment as every other ssh this transport starts; stdin
    /// is closed, stdout and stderr are piped, and the process dies with its handle
    /// (`kill_on_drop`). It is the caller's job to bound how long it reads — the placed-run
    /// relay ([`crate::relay`]) only ever relays bytes a pane's child already wrote to disk, so
    /// losing this process loses the view of the child, never the child.
    ///
    /// # Errors
    /// The ssh process could not be started.
    pub fn stream(
        &self,
        target: &str,
        command: &str,
    ) -> Result<tokio::process::Child, RemoteError> {
        let mut process = self.command();
        process
            .args(self.args())
            .arg(target)
            .arg(command)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        process
            .spawn()
            .map_err(|error| RemoteError(format!("ssh {target} could not start: {error}")))
    }

    /// `discoverHerdrEndpoint(machine)` (`herdr-connection.ts:62-68`): ask the machine's own herdr
    /// where its API socket is, under its saved session.
    ///
    /// # Errors
    /// pi's discovery sentences, then [`parse_endpoint`]'s.
    pub async fn discover_endpoint(
        &self,
        target: &str,
        session: Option<&str>,
    ) -> Result<RemoteEndpoint, RemoteError> {
        let discovery = "herdr_path=$(command -v herdr) || exit 127; case \"$herdr_path\" in /*/herdr) ;; *) exit 126;; esac; exec \"$herdr_path\" status server --json";
        let result = self
            .run(
                target,
                &ssh_env_command(session, discovery),
                REMOTE_COMMAND_TIMEOUT,
                MAX_DISCOVERY_BYTES,
                None,
            )
            .await;
        if let Some(error) = result.error {
            return Err(RemoteError(format!(
                "Remote Herdr discovery failed: {error}"
            )));
        }
        if result.status != Some(0) {
            let detail = if result.stderr.is_empty() {
                result.stdout.trim()
            } else {
                result.stderr.trim()
            };
            return Err(RemoteError(format!(
                "Remote Herdr discovery failed with code {}: {detail}",
                result
                    .status
                    .map_or_else(|| "null".to_string(), |code| code.to_string())
            )));
        }
        parse_endpoint(&result.stdout, session)
    }

    /// `connectHerdrMachine(machine)` (`herdr-connection.ts:119-133`): discover the endpoint, make
    /// a private local directory, forward the remote API socket into it, and hand back a
    /// [`HerdrClient`] on the forwarded path.
    ///
    /// # Errors
    /// Discovery's errors, or the forward's.
    pub async fn connect(
        &self,
        target: &str,
        session: Option<&str>,
    ) -> Result<MachineConnection, RemoteError> {
        let endpoint = self.discover_endpoint(target, session).await?;
        let dir = private_temp_dir().map_err(|error| {
            RemoteError(format!(
                "Could not create a private local directory for the Herdr forward: {error}"
            ))
        })?;
        let mut connection = MachineConnection {
            endpoint,
            client: HerdrClient::new(dir.join("herdr.sock")),
            transport: self.clone(),
            target: target.to_string(),
            dir,
            forwards: Vec::new(),
        };
        let remote = connection.endpoint.socket.clone();
        match connection.forward_remote_socket(&remote, "herdr").await {
            Ok(local) => {
                connection.client = HerdrClient::new(local);
                Ok(connection)
            }
            Err(error) => {
                connection.close().await;
                Err(error)
            }
        }
    }

    /// Spawn `ssh -N -o StreamLocalBindUnlink=yes -L <local>:<remote> <target>` and wait for the
    /// local socket (`forward`, `herdr-connection.ts:123-130`).
    async fn spawn_forward(
        &self,
        target: &str,
        local: &Path,
        remote: &str,
    ) -> Result<tokio::process::Child, RemoteError> {
        let mut process = self.command();
        process
            .args(self.args())
            .args(["-N", "-o", "StreamLocalBindUnlink=yes", "-L"])
            .arg(format!("{}:{remote}", local.display()))
            .arg(target)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = process.spawn().map_err(|error| {
            RemoteError(format!(
                "SSH StreamLocal forwarding could not start: {error}"
            ))
        })?;
        match wait_for_socket(local, &mut child).await {
            Ok(()) => Ok(child),
            Err(error) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                let _ = std::fs::remove_file(local);
                Err(error)
            }
        }
    }
}

/// `waitForSocket(socketPath, child)` (`herdr-connection.ts:108-115`): up to 100 polls 25 ms
/// apart; the forward exiting first is its own error; the socket is chmodded 0600 once it exists.
///
/// `metadata` (following a link) rather than upstream's `lstat`: the directory is this process's
/// own 0700 one, so only this uid could have placed a link there, and a link to a socket is as good
/// as the socket for `connect(2)`.
async fn wait_for_socket(
    path: &Path,
    child: &mut tokio::process::Child,
) -> Result<(), RemoteError> {
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};
    for _ in 0..100 {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(RemoteError(format!(
                "SSH StreamLocal forwarding exited with code {}.",
                status
                    .code()
                    .map_or_else(|| "null".to_string(), |code| code.to_string())
            )));
        }
        if let Ok(meta) = std::fs::metadata(path)
            && meta.file_type().is_socket()
        {
            if std::fs::symlink_metadata(path).is_ok_and(|link| link.file_type().is_socket()) {
                let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
            }
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    Err(RemoteError(
        "SSH StreamLocal forwarding did not create its local socket.".to_string(),
    ))
}

/// A fresh mode-0700 directory under the system temp dir (`fs.mkdtempSync(…, "pi-subagents-herdr-")`
/// then `chmodSync(dir, 0o700)`, `herdr-connection.ts:121`).
fn private_temp_dir() -> std::io::Result<PathBuf> {
    use std::os::unix::fs::DirBuilderExt;
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let base = std::env::temp_dir();
    for _ in 0..16 {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.subsec_nanos());
        let name = format!(
            "cyrup-herdr-{}-{}-{nonce:08x}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let dir = base.join(name);
        match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => return Ok(dir),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::other(
        "could not pick an unused private directory name",
    ))
}

/// A live connection to one saved machine's herdr (`HerdrForwardedConnection`, `:22`).
///
/// Owns its `-N` forwards and its private directory; [`Self::close`] stops every forward and
/// removes the directory. Dropping without closing still kills the forwards
/// (`kill_on_drop`), but leaves the directory — close it.
#[derive(Debug)]
pub struct MachineConnection {
    /// What the machine's herdr reported about itself.
    pub endpoint: RemoteEndpoint,
    /// The remote herdr, through the forwarded socket.
    pub client: HerdrClient,
    transport: SshTransport,
    target: String,
    dir: PathBuf,
    forwards: Vec<(PathBuf, tokio::process::Child)>,
}

impl MachineConnection {
    /// The ssh transport this connection was made through.
    #[must_use]
    pub fn transport(&self) -> &SshTransport {
        &self.transport
    }

    /// The private local directory the forwards live in.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Forward one more remote socket into this connection's directory as `<name>.sock`
    /// (`forwardRemoteSocket`, `herdr-connection.ts:123-130`).
    ///
    /// # Errors
    /// A relative `remote_path`, or the forward not coming up.
    pub async fn forward_remote_socket(
        &mut self,
        remote_path: &str,
        name: &str,
    ) -> Result<PathBuf, RemoteError> {
        if !remote_path.starts_with('/') {
            return Err(RemoteError(
                "Remote socket path must be absolute.".to_string(),
            ));
        }
        let local = self.dir.join(format!("{name}.sock"));
        let child = self
            .transport
            .spawn_forward(&self.target, &local, remote_path)
            .await?;
        self.forwards.push((local.clone(), child));
        Ok(local)
    }

    /// Whether this connection's transport is gone: any `-N` forward has exited. This is
    /// upstream's `connection: "unknown"` (`markConnectionUnknown`, `herdr-placed-run.ts:168`
    /// @v0.68.0), which pi learns from its herdr subscription erroring; cyrup's herdr client opens
    /// one connection per request, so the forward's own process is the observable that dies with
    /// the transport.
    pub fn is_lost(&mut self) -> bool {
        self.forwards
            .iter_mut()
            .any(|(_, child)| !matches!(child.try_wait(), Ok(None)))
    }

    /// Stop every forward and remove the directory (`close`, `herdr-connection.ts:132`).
    pub async fn close(mut self) {
        for (path, mut child) in self.forwards.drain(..) {
            let _ = child.start_kill();
            let _ = child.wait().await;
            let _ = std::fs::remove_file(path);
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
