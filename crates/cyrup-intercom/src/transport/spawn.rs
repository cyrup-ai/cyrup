//! Broker discovery + auto-spawn — a port of `pi-intercom/broker/spawn.ts:179-387`.
//!
//! Discovery is **transport-agnostic**: `check_target_connectable` probes whichever
//! [`crate::transport::target::BrokerConnectTarget`] the platform + environment selects (Unix
//! socket, Windows named pipe, or the opt-in loopback TCP endpoint), and echoes the endpoint's
//! `stateId` credential when the target is TCP (`spawn.ts:274,288-291`). `ensure_broker` re-resolves
//! that target on **every** probe, exactly as pi's argument-less `checkSocketConnectable()` re-calls
//! `getBrokerConnectTarget()` (`spawn.ts:268-273`) — under TCP the target does not exist until the
//! broker has published `broker.port.json`, so a once-resolved, cached target could never become
//! connectable.
//!
//! `ensure_broker` ([`spawnBrokerIfNeeded`]) is idempotent: if the broker is already health-
//! connectable it returns; otherwise it takes an exclusive spawn lock (`O_EXCL`), re-checks, and
//! **re-execs the broker as a detached OS process** — the configured `broker_command`/`broker_args`
//! (`IntercomConfig`, `config.ts:24,26`) if they differ from pi's own `"npx" ["--no-install","tsx"]`
//! default, else `current_exe()` (or the `CYRUP_INTERCOM_BROKER_BINARY` override) with argv
//! `["__intercom-broker"]` — mirroring `getBrokerLaunchSpec`'s `usesDefaultBrokerCommand` gate
//! (`spawn.ts:67-72,121-154`) and `ensureConnected`'s `spawnBrokerIfNeeded(config.brokerCommand,
//! config.brokerArgs)` call (`index.ts:828`). Stdin/stdout null and stderr PIPED into a bounded 4 KB
//! tail (`v0.10.1 broker/spawn.ts:25,156-176,216-232`, commit `c9675a5`) so a startup failure can
//! say why instead of only `exited before startup with code 1`; `process_group(0)` (the unsafe-free analog
//! of pi's `detached:true`, mirroring `cyrup-ext-subagents`' `spawn_detached_runner`). The spawned
//! child is then raced against `wait_for_broker` (5 s, `spawn.ts:205-237`): an early exit/error
//! surfaces immediately as a descriptive [`IntercomError::Broker`] instead of waiting out the full
//! timeout. The lock is released in a `finally`-equivalent on every path.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::error::{IntercomError, Result};
use crate::paths;
use crate::transport::framing::{FrameReader, encode_json};
use crate::transport::protocol::{HealthMessage, PROTOCOL_NAME, PROTOCOL_VERSION, now_ms};
use crate::transport::stream::BrokerStream;
use crate::transport::target::{self, BrokerConnectTarget};

/// The hidden subcommand the broker re-exec appends (mirrors `__subagent-runner`).
pub const INTERCOM_BROKER_SUBCOMMAND: &str = "__intercom-broker";
/// The broker-binary override (mirrors `CYRUP_SUBAGENT_BINARY`; pi `brokerCommand`).
pub const ENV_INTERCOM_BROKER_BINARY: &str = "CYRUP_INTERCOM_BROKER_BINARY";

const HEALTH_TIMEOUT: Duration = Duration::from_secs(1);
const WAIT_FOR_BROKER_TIMEOUT: Duration = Duration::from_secs(5);
const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(100);
const SPAWN_LOCK_STALE_MS: u64 = 10_000;
const SPAWN_LOCK_MAX_RETRIES: u32 = 5;

/// `spawnBrokerIfNeeded` (`spawn.ts:179-241`): ensure a broker is running for `agent_dir`, spawning
/// one (detached) if none is health-connectable. Idempotent and safe to call from every session.
///
/// # Errors
/// [`IntercomError::Broker`] if the broker could not be spawned or did not become healthy in time.
pub async fn ensure_broker(agent_dir: &Path) -> Result<()> {
    let intercom_dir = paths::intercom_dir_path(agent_dir);
    paths::ensure_intercom_runtime_dir(&intercom_dir).map_err(|e| IntercomError::Broker(e.to_string()))?;
    let pid_path = paths::broker_pid_path(&intercom_dir);
    let lock_path = paths::broker_spawn_lock_path(&intercom_dir);
    // `ensureConnected` passes `config.brokerCommand`/`config.brokerArgs` straight through to
    // `spawnBrokerIfNeeded` (`index.ts:828`) — load the same config here so a user override
    // genuinely changes what gets launched below, instead of being silently ignored.
    let config = crate::config::load_config(&intercom_dir).map_err(IntercomError::Broker)?;

    if is_broker_running_for(agent_dir, &pid_path).await {
        return Ok(());
    }

    if !acquire_spawn_lock(&lock_path) {
        // Another process is spawning — just wait for it (spawn.ts:187-190).
        return wait_for_broker_for(agent_dir, WAIT_FOR_BROKER_TIMEOUT).await;
    }

    // Owner path — release the lock on every exit (spawn.ts:238-240).
    let result = spawn_owner(agent_dir, &pid_path, &config.broker_command, &config.broker_args).await;
    release_spawn_lock(&lock_path);
    result
}

async fn spawn_owner(
    agent_dir: &Path,
    pid_path: &Path,
    broker_command: &str,
    broker_args: &[String],
) -> Result<()> {
    // Re-check now that we hold the lock (spawn.ts:193-195).
    if is_broker_running_for(agent_dir, pid_path).await {
        return Ok(());
    }
    let (mut child, stderr_tail) = spawn_detached_broker(agent_dir, broker_command, broker_args)?;
    // Race the health-poll against the child's own exit (spawn.ts:205-236): a broker that fails to
    // spawn or dies before startup completes must fail fast with its exit code/signal, not silently
    // wait out the full 5s timeout only to report a generic "timed out".
    let result = tokio::select! {
        res = wait_for_broker_for(agent_dir, WAIT_FOR_BROKER_TIMEOUT) => res,
        wait = child.wait() => Err(broker_wait_error(wait)),
    };
    // Upstream switched its exit listener from `exit` to `close` in the same commit (`c9675a5`,
    // `v0.10.1 broker/spawn.ts:262,271`) precisely so the stderr pipe has DRAINED before the error
    // is built. `child.wait()` returns on process exit, which is `exit`, not `close`; give the drain
    // task a bounded moment to reach EOF so the tail is not empty by a scheduling accident.
    let result = match result {
        Ok(()) => Ok(()),
        Err(e) => {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            // `brokerStartupError` decorates EVERY rejection path upstream (`:243`, `:254`, `:258`,
            // `:267-268`) — the spawn error, both exit arms and the `waitForBroker` timeout — so a
            // health-poll timeout gets the broker's reason too, not just an early exit.
            Err(stderr_tail.decorate(e))
        }
    };
    // The child keeps running under its own detached process group regardless of which branch of
    // the race won; dropping the handle here does not kill it (mirrors pi's `child.unref()`).
    drop(child);
    result
}

/// `usesDefaultBrokerCommand` (`spawn.ts:67-72`): whether `broker_command`/`broker_args` are still
/// pi's own literal default (`"npx"`, `["--no-install","tsx"]`) — i.e. the user never configured a
/// custom broker launch.
fn uses_default_broker_command(broker_command: &str, broker_args: &[String]) -> bool {
    broker_command == "npx" && broker_args == ["--no-install", "tsx"]
}

/// Resolve which binary to re-exec + its argv (`getBrokerLaunchSpec`, `spawn.ts:121-154`, cyrup
/// re-exec form). Mirrors pi's `usesDefaultBrokerCommand` gate: the `CYRUP_INTERCOM_BROKER_BINARY`
/// override always wins; otherwise, while `broker_command`/`broker_args` are still pi's own default,
/// re-exec `current_exe()` (else the literal `"cyrup"`) with the `__intercom-broker` subcommand; a
/// genuinely user-configured `broker_command`/`broker_args` is honored verbatim, with the
/// `__intercom-broker` subcommand appended as the final arg (cyrup's re-exec analog of pi appending
/// `brokerPath`, `spawn.ts:149-153`).
#[must_use]
pub fn resolve_broker_command(broker_command: &str, broker_args: &[String]) -> (PathBuf, Vec<String>) {
    if let Ok(bin) = std::env::var(ENV_INTERCOM_BROKER_BINARY)
        && !bin.trim().is_empty()
    {
        return (PathBuf::from(bin), vec![INTERCOM_BROKER_SUBCOMMAND.to_string()]);
    }
    if uses_default_broker_command(broker_command, broker_args) {
        let binary = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("cyrup"));
        return (binary, vec![INTERCOM_BROKER_SUBCOMMAND.to_string()]);
    }
    let mut args = broker_args.to_vec();
    args.push(INTERCOM_BROKER_SUBCOMMAND.to_string());
    (PathBuf::from(broker_command), args)
}

/// Spawn the broker as a genuinely detached process (`spawn.ts:202-203`, cyrup form): stdio null,
/// `process_group(0)`, env overlay `CYRUP_CODING_AGENT_DIR=<abs agent dir>`. The child handle is
/// returned (rather than dropped) so the caller can race it against `wait_for_broker` (spawn.ts:205-
/// 236); it keeps running detached under its own process group however the caller's handle is
/// eventually dropped. Mirrors `cyrup-ext-subagents::background::spawn_detached_runner`.
fn spawn_detached_broker(
    agent_dir: &Path,
    broker_command: &str,
    broker_args: &[String],
) -> Result<(tokio::process::Child, BrokerStderrTail)> {
    let (binary, args) = resolve_broker_command(broker_command, broker_args);
    let mut command = tokio::process::Command::new(&binary);
    command
        .args(&args)
        .env(paths::ENV_CODING_AGENT_DIR, agent_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        // `getBrokerSpawnOptions(extensionDir, env, captureStderr)` switches stdio to
        // `["ignore","ignore","pipe"]` (`v0.10.1 broker/spawn.ts:156-176`, commit `c9675a5`).
        // stdout stays null; only stderr is piped, and only so a startup failure can say WHY.
        .stderr(std::process::Stdio::piped());
    #[cfg(unix)]
    {
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        // The Windows analog of `getBrokerSpawnOptions`' `detached: true` + `windowsHide: true`
        // (`spawn.ts:167,171`). pi reaches the same end by writing a `.vbs` launcher and running it
        // through `wscript.exe` with a hidden window (`getWindowsHiddenLauncherScript` /
        // `getBrokerLaunchSpec`, `spawn.ts:88-95,130-139`), because Node cannot pass raw creation
        // flags. cyrup re-execs its own binary directly (see `resolve_broker_command`), so the
        // launcher script has nothing to launch — the same detach + no-console-window outcome is
        // requested from the OS directly instead. DETACHED_PROCESS (0x8) is the counterpart of the
        // `process_group(0)` used on unix above; CREATE_NO_WINDOW (0x0800_0000) is `windowsHide`.
        // `creation_flags` is `tokio::process::Command`'s own windows-only inherent method (mirroring
        // `process_group`), so no `std::os::windows::process::CommandExt` import is involved.
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW);
    }
    let mut child = command.spawn().map_err(|e| IntercomError::Broker(format!("failed to spawn intercom broker: {e}")))?;
    let tail = BrokerStderrTail::drain(child.stderr.take());
    Ok((child, tail))
}

/// `BROKER_STARTUP_STDERR_LIMIT = 4_000` (`v0.10.1 broker/spawn.ts:25`) — how many trailing bytes of
/// the broker's stderr are kept for the startup error message.
const BROKER_STARTUP_STDERR_LIMIT: usize = 4_000;

/// The last [`BROKER_STARTUP_STDERR_LIMIT`] bytes the detached broker wrote to stderr
/// (`rememberBrokerStderr`, `v0.10.1 broker/spawn.ts:216-218`).
///
/// The drain task runs to EOF, not just for the startup window. That is upstream's
/// `child.stderr?.resume()` in `cleanup` (`:228`): a piped stderr nobody reads fills its pipe buffer
/// and BLOCKS the long-lived broker on its next write. Reading to EOF with a bounded tail keeps
/// memory constant and the broker unblocked.
#[derive(Clone)]
pub(crate) struct BrokerStderrTail {
    buffer: std::sync::Arc<std::sync::Mutex<String>>,
}

impl BrokerStderrTail {
    fn drain(stderr: Option<tokio::process::ChildStderr>) -> Self {
        let buffer = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        if let Some(mut stderr) = stderr {
            let sink = buffer.clone();
            tokio::spawn(async move {
                use tokio::io::AsyncReadExt;
                let mut chunk = [0u8; 1024];
                loop {
                    match stderr.read(&mut chunk).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let text = String::from_utf8_lossy(chunk.get(..n).unwrap_or_default());
                            let mut guard = sink.lock().unwrap_or_else(|e| e.into_inner());
                            guard.push_str(&text);
                            // `.slice(-LIMIT)` — keep the TAIL, on a char boundary so the buffer
                            // stays valid UTF-8.
                            if guard.len() > BROKER_STARTUP_STDERR_LIMIT {
                                let cut = guard.len() - BROKER_STARTUP_STDERR_LIMIT;
                                let cut = (cut..guard.len())
                                    .find(|i| guard.is_char_boundary(*i))
                                    .unwrap_or(guard.len());
                                *guard = guard.split_at(cut).1.to_string();
                            }
                        }
                    }
                }
            });
        }
        Self { buffer }
    }

    /// `brokerStartupError(message, cause)` (`v0.10.1 broker/spawn.ts:219-223`): append
    /// `\nBroker stderr:\n{stderr}` when the captured tail is non-empty.
    fn decorate(&self, error: IntercomError) -> IntercomError {
        let tail = self.buffer.lock().unwrap_or_else(|e| e.into_inner()).trim().to_string();
        if tail.is_empty() {
            return error;
        }
        IntercomError::Broker(format!("{error}\nBroker stderr:\n{tail}"))
    }
}

/// Turn a completed/failed `child.wait()` into the descriptive early-failure error pi raises from
/// its `exit`/`error` handlers (`spawn.ts:211-226`): `Failed to spawn intercom broker: <msg>` if the
/// wait itself errored, else `Intercom broker exited before startup with signal <sig>` / `... with
/// code <code>` (`unknown` if neither is available).
fn broker_wait_error(wait_result: std::io::Result<std::process::ExitStatus>) -> IntercomError {
    let status = match wait_result {
        Ok(status) => status,
        Err(e) => return IntercomError::Broker(format!("failed to spawn intercom broker: {e}")),
    };
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            let name = nix::sys::signal::Signal::try_from(sig)
                .map_or_else(|_| sig.to_string(), |s| s.to_string());
            return IntercomError::Broker(format!("intercom broker exited before startup with signal {name}"));
        }
    }
    match status.code() {
        Some(code) => IntercomError::Broker(format!("intercom broker exited before startup with code {code}")),
        None => IntercomError::Broker("intercom broker exited before startup with code unknown".to_string()),
    }
}

/// `isBrokerRunning` (`spawn.ts:243-259`) against an explicit socket/pipe path.
pub async fn is_broker_running(socket_path: &Path, pid_path: &Path) -> bool {
    is_broker_running_target(&BrokerConnectTarget::Socket(socket_path.to_path_buf()), pid_path).await
}

/// `isBrokerRunning` (`spawn.ts:243-259`): a health-connectable target, OR a live pid file
/// (`broker.pid` exists, `kill(pid,0)` succeeds) plus a connectable target.
pub async fn is_broker_running_target(target: &BrokerConnectTarget, pid_path: &Path) -> bool {
    if check_target_connectable(target).await {
        return true;
    }
    let Ok(pid_raw) = std::fs::read_to_string(pid_path) else {
        return false;
    };
    let Ok(pid) = pid_raw.trim().parse::<i32>() else {
        return false;
    };
    if !pid_alive(pid) {
        return false;
    }
    check_target_connectable(target).await
}

/// `isBrokerRunning` with the target **re-resolved** from `agent_dir` + the process env, exactly as
/// pi's argument-less `checkSocketConnectable()` re-calls `getBrokerConnectTarget()` on every
/// invocation (`spawn.ts:268-273`). Under the opt-in TCP transport the target does not exist until
/// the broker has published `broker.port.json`, so it must not be resolved once and cached.
pub async fn is_broker_running_for(agent_dir: &Path, pid_path: &Path) -> bool {
    match target::broker_connect_target(agent_dir) {
        Ok(t) => is_broker_running_target(&t, pid_path).await,
        // `getBrokerConnectTarget()` throwing (no/short/invalid `broker.port.json`) is caught and
        // resolves `false` — "not connectable" (`spawn.ts:269-273`).
        Err(_) => false,
    }
}

/// `checkSocketConnectable` (`spawn.ts:267-313`) against an explicit socket/pipe path.
pub async fn check_socket_connectable(socket_path: &Path) -> bool {
    check_target_connectable(&BrokerConnectTarget::Socket(socket_path.to_path_buf())).await
}

/// `checkSocketConnectable` (`spawn.ts:267-313`): connect to `target`, send a `health` probe, and
/// require a byte-identical `health_ok` (`{protocol:"pi-intercom",version:1}`) within 1 s.
///
/// Over a TCP target the probe carries the endpoint's `stateId`
/// (`spawn.ts:274,288-291`: `...(expectedStateId ? { stateId: expectedStateId } : {})`); without it
/// the broker rejects the probe with `Invalid intercom TCP endpoint credentials`
/// (`broker.ts:251-254`) and the connection is not considered healthy. Over a socket/pipe target the
/// field is **omitted**, not sent as null.
pub async fn check_target_connectable(target: &BrokerConnectTarget) -> bool {
    let request_id = uuid::Uuid::new_v4().to_string();
    let state_id = target.state_id().map(str::to_string);
    let probe = async {
        let mut stream = BrokerStream::connect(target).await.ok()?;
        let frame = encode_json(&HealthMessage::Health {
            request_id: request_id.clone(),
            state_id,
        })
        .ok()?;
        stream.write_all(&frame).await.ok()?;
        let mut reader = FrameReader::new();
        let mut buf = vec![0u8; 4096];
        loop {
            let n = stream.read(&mut buf).await.ok()?;
            if n == 0 {
                return None;
            }
            let chunk = buf.get(..n).unwrap_or(&[]);
            let frames = reader.push(chunk).ok()?;
            for payload in frames {
                if let Ok(HealthMessage::HealthOk { request_id: rid, protocol, version }) =
                    serde_json::from_slice::<HealthMessage>(&payload)
                    && rid == request_id
                    && protocol == PROTOCOL_NAME
                    && version == PROTOCOL_VERSION
                {
                    return Some(());
                }
            }
        }
    };
    matches!(tokio::time::timeout(HEALTH_TIMEOUT, probe).await, Ok(Some(())))
}

/// `waitForBroker` (`spawn.ts:378-387`) against an explicit socket/pipe path.
///
/// # Errors
/// [`IntercomError::Broker`] if the broker did not become connectable within `timeout`.
pub async fn wait_for_broker(socket_path: &Path, timeout: Duration) -> Result<()> {
    let target = BrokerConnectTarget::Socket(socket_path.to_path_buf());
    wait_until(timeout, || check_target_connectable(&target)).await
}

/// `waitForBroker` (`spawn.ts:378-387`) with the target **re-resolved** every poll from `agent_dir`
/// plus the process env. See [`is_broker_running_for`] for why caching it would break the TCP
/// transport: the endpoint file appears only once the broker is listening (`broker.ts:64-81`).
///
/// # Errors
/// [`IntercomError::Broker`] if the broker did not become connectable within `timeout`.
pub async fn wait_for_broker_for(agent_dir: &Path, timeout: Duration) -> Result<()> {
    wait_until(timeout, || async {
        match target::broker_connect_target(agent_dir) {
            Ok(t) => check_target_connectable(&t).await,
            Err(_) => false,
        }
    })
    .await
}

/// The shared 100 ms poll ladder (`spawn.ts:378-387`).
async fn wait_until<F, Fut>(timeout: Duration, mut probe: F) -> Result<()>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let start = tokio::time::Instant::now();
    while start.elapsed() < timeout {
        if probe().await {
            return Ok(());
        }
        tokio::time::sleep(WAIT_POLL_INTERVAL).await;
    }
    Err(IntercomError::Broker("broker failed to start within timeout".to_string()))
}

/// `acquireSpawnLock` (`spawn.ts:315-341`): exclusive-create `broker.spawn.lock` (`O_EXCL`) with body
/// `"<pid>\n<now>\n"`. On EEXIST: a stale lock (dead creator pid, or age > 10 s) is unlinked + retried
/// (≤5); a live lock returns `false` (this process is not the spawn owner).
fn acquire_spawn_lock(lock_path: &Path) -> bool {
    for _ in 0..SPAWN_LOCK_MAX_RETRIES {
        match std::fs::OpenOptions::new().write(true).create_new(true).open(lock_path) {
            Ok(mut file) => {
                use std::io::Write;
                let body = format!("{}\n{}\n", std::process::id(), now_ms());
                let _ = file.write_all(body.as_bytes());
                let _ = paths::restrict_intercom_runtime_file(lock_path);
                return true;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if is_spawn_lock_stale(lock_path) {
                    let _ = std::fs::remove_file(lock_path);
                    continue;
                }
                return false;
            }
            Err(_) => return false,
        }
    }
    false
}

/// `isSpawnLockStale` (`spawn.ts:343-368`): the lock's creator pid is gone, or its age exceeds 10 s.
fn is_spawn_lock_stale(lock_path: &Path) -> bool {
    let Ok(contents) = std::fs::read_to_string(lock_path) else {
        // Unreadable lock contents are treated as stale (spawn.ts:364-366).
        return true;
    };
    let mut lines = contents.trim().lines();
    let pid = lines.next().and_then(|l| l.trim().parse::<i32>().ok());
    let created_at = lines.next().and_then(|l| l.trim().parse::<u64>().ok());

    if let Some(pid) = pid
        && !pid_alive(pid)
    {
        return true;
    }
    match created_at {
        None => true,
        Some(created) => now_ms().saturating_sub(created) > SPAWN_LOCK_STALE_MS,
    }
}

fn release_spawn_lock(lock_path: &Path) {
    let _ = std::fs::remove_file(lock_path);
}

/// `process.kill(pid, 0)` liveness (`spawn.ts:253,356`) via `nix::sys::signal::kill(pid, None)`.
fn pid_alive(pid: i32) -> bool {
    #[cfg(unix)]
    {
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
    use super::*;
    use crate::transport::target::{BrokerTcpEndpoint, INTERCOM_TCP_HOST};

    /// Read length-prefixed frames off `stream` until the first one arrives, answer it with
    /// `reply`, and hand the probe frame back to the test as raw JSON — a broker stand-in narrow
    /// enough to assert the exact bytes `checkSocketConnectable` puts on the wire
    /// (`spawn.ts:286-292`).
    async fn answer_one_probe<S>(mut stream: S, reply: serde_json::Value) -> serde_json::Value
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        let mut reader = FrameReader::new();
        let mut buf = vec![0u8; 4096];
        loop {
            let n = stream.read(&mut buf).await.unwrap();
            assert_ne!(n, 0, "the prober closed without sending a health frame");
            if let Some(payload) = reader.push(&buf[..n]).unwrap().into_iter().next() {
                let probe: serde_json::Value = serde_json::from_slice(&payload).unwrap();
                let mut reply = reply.clone();
                if let Some(obj) = reply.as_object_mut() {
                    obj.insert("requestId".to_string(), probe["requestId"].clone());
                }
                stream.write_all(&encode_json(&reply).unwrap()).await.unwrap();
                return probe;
            }
        }
    }

    fn health_ok_reply() -> serde_json::Value {
        serde_json::json!({ "type": "health_ok", "protocol": PROTOCOL_NAME, "version": PROTOCOL_VERSION })
    }

    /// `spawn.ts:274,288-291` — over a TCP target the health probe MUST carry the endpoint's
    /// `stateId`, or the broker rejects it as `Invalid intercom TCP endpoint credentials`
    /// (`broker.ts:251-254`) and discovery can never succeed. Runs over a real loopback
    /// `TcpListener` bound to `127.0.0.1:0` — no network.
    #[tokio::test]
    async fn health_probe_over_tcp_carries_the_endpoint_state_id() {
        let listener = tokio::net::TcpListener::bind((INTERCOM_TCP_HOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let broker = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            answer_one_probe(stream, health_ok_reply()).await
        });

        let target = BrokerConnectTarget::Tcp(BrokerTcpEndpoint {
            host: INTERCOM_TCP_HOST.to_string(),
            port,
            state_id: Some("state-1".to_string()),
        });
        assert!(check_target_connectable(&target).await, "a valid health_ok means connectable");

        let probe = broker.await.unwrap();
        assert_eq!(probe["type"], "health");
        assert_eq!(probe["stateId"], "state-1", "spawn.ts:290 spreads the endpoint stateId");
        assert!(probe["requestId"].is_string());
    }

    // Unix-domain-socket specific (`UnixStream::pair()` / `UnixListener`): the transport-neutral
    // behaviour it asserts is covered on Windows by the named-pipe arm of `broker::listener`.
    #[cfg(unix)]
    /// MIRROR (stays green): over a socket target pi spreads `{}` instead
    /// (`...(expectedStateId ? { stateId } : {})`, `spawn.ts:290`) — the key must be **absent**, not
    /// null, since the broker compares `clientMessage.stateId === BROKER_STATE_ID`.
    #[tokio::test]
    async fn health_probe_over_a_socket_omits_the_state_id() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("broker.sock");
        let listener = tokio::net::UnixListener::bind(&socket_path).unwrap();
        let broker = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            answer_one_probe(stream, health_ok_reply()).await
        });

        assert!(check_socket_connectable(&socket_path).await);
        let probe = broker.await.unwrap();
        assert_eq!(probe["type"], "health");
        assert!(probe.get("stateId").is_none(), "socket probes carry no credential: {probe}");
    }

    /// `isBrokerHealthOkMessage` (`spawn.ts:97-106`) — a reply whose `protocol` is not
    /// `pi-intercom` is not a healthy broker, over TCP just as over a socket.
    #[tokio::test]
    async fn tcp_health_probe_rejects_a_foreign_protocol_reply() {
        let listener = tokio::net::TcpListener::bind((INTERCOM_TCP_HOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            answer_one_probe(
                stream,
                serde_json::json!({ "type": "health_ok", "protocol": "something-else", "version": 1 }),
            )
            .await
        });

        let target = BrokerConnectTarget::Tcp(BrokerTcpEndpoint {
            host: INTERCOM_TCP_HOST.to_string(),
            port,
            state_id: Some("state-1".to_string()),
        });
        assert!(!check_target_connectable(&target).await);
    }

    /// `spawn.ts:268-273` — `getBrokerConnectTarget()` throwing (here: the TCP transport is opted
    /// in but `broker.port.json` does not exist yet) is caught and resolves `false`, so
    /// `ensure_broker`'s discovery treats it as "no broker" and proceeds, rather than propagating.
    #[tokio::test]
    async fn discovery_treats_an_unresolvable_target_as_not_connectable() {
        let dir = tempfile::tempdir().unwrap();
        let pid_path = dir.path().join("broker.pid");
        // No broker, no socket, no pid file: the socket target simply is not connectable.
        assert!(!is_broker_running_for(dir.path(), &pid_path).await);
        assert!(
            wait_for_broker_for(dir.path(), Duration::from_millis(250)).await.is_err(),
            "the poll ladder must time out, not hang or panic"
        );
    }

    #[test]
    fn resolve_broker_command_appends_subcommand() {
        let (_bin, args) = resolve_broker_command("npx", &["--no-install".to_string(), "tsx".to_string()]);
        assert_eq!(args, vec![INTERCOM_BROKER_SUBCOMMAND.to_string()]);
    }

    /// Regression for the dossier item: a configured `brokerCommand`/`brokerArgs` (`config.ts:24,26`)
    /// must genuinely change what gets launched (`usesDefaultBrokerCommand`, `spawn.ts:67-72,149-153`)
    /// instead of being silently ignored in favor of always re-execing `current_exe()`.
    #[test]
    fn resolve_broker_command_honors_configured_custom_command() {
        let (bin, args) = resolve_broker_command("my-custom-broker", &["--flag".to_string()]);
        assert_eq!(bin, PathBuf::from("my-custom-broker"));
        assert_eq!(args, vec!["--flag".to_string(), INTERCOM_BROKER_SUBCOMMAND.to_string()]);
    }

    #[test]
    fn uses_default_broker_command_matches_pis_literal_default() {
        assert!(uses_default_broker_command("npx", &["--no-install".to_string(), "tsx".to_string()]));
        assert!(!uses_default_broker_command("npx", &["--no-install".to_string()]));
        assert!(!uses_default_broker_command("yarn", &["--no-install".to_string(), "tsx".to_string()]));
    }

    /// Regression for the dossier item: a broker that dies immediately on startup must be reported
    /// fast, with its exit code, rather than only after the full 5s `wait_for_broker` timeout.
    #[tokio::test]
    async fn spawn_owner_detects_early_exit_without_waiting_full_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let intercom_dir = dir.path().join("intercom");
        std::fs::create_dir_all(&intercom_dir).unwrap();
        let pid_path = intercom_dir.join("broker.pid");

        let start = std::time::Instant::now();
        // "false" is a non-default broker_command (per `uses_default_broker_command`), so this also
        // exercises the custom-command path landing on a real, immediately-exiting process.
        let result = spawn_owner(dir.path(), &pid_path, "false", &[]).await;
        let elapsed = start.elapsed();

        assert!(result.is_err(), "a broker that exits immediately must be reported as an error");
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("exited before startup"), "error should describe the early exit: {msg}");
        assert!(
            elapsed < Duration::from_secs(2),
            "must fail fast on early exit, not wait the full 5s timeout: {elapsed:?}"
        );
    }

    #[test]
    fn spawn_lock_acquire_then_contended_then_release() {
        let dir = tempfile::tempdir().unwrap();
        let lock = dir.path().join("broker.spawn.lock");

        // First acquire succeeds.
        assert!(acquire_spawn_lock(&lock));
        assert!(lock.exists());

        // A live, fresh lock (this process's own pid, just written) is not stale → contended.
        assert!(!is_spawn_lock_stale(&lock));
        assert!(!acquire_spawn_lock(&lock), "a live lock must not be re-acquired");

        release_spawn_lock(&lock);
        assert!(!lock.exists());
        // After release, acquire succeeds again.
        assert!(acquire_spawn_lock(&lock));
        release_spawn_lock(&lock);
    }

    #[test]
    fn stale_lock_with_dead_pid_is_reclaimed() {
        let dir = tempfile::tempdir().unwrap();
        let lock = dir.path().join("broker.spawn.lock");
        // pid 0 is never a real user process → `kill(0, None)` targets the whole group, so use a pid
        // that cannot be alive: a very large pid unlikely to exist.
        std::fs::write(&lock, format!("2147483646\n{}\n", now_ms())).unwrap();
        assert!(is_spawn_lock_stale(&lock), "a lock whose creator pid is gone is stale");
    }

    #[test]
    fn stale_lock_by_age_is_reclaimed() {
        let dir = tempfile::tempdir().unwrap();
        let lock = dir.path().join("broker.spawn.lock");
        // Our own live pid but a very old timestamp → stale by age (> 10s).
        std::fs::write(&lock, format!("{}\n{}\n", std::process::id(), now_ms().saturating_sub(20_000))).unwrap();
        assert!(is_spawn_lock_stale(&lock), "a lock older than 10s is stale even with a live pid");
    }
}
