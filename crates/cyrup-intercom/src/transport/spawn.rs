//! Broker discovery + auto-spawn — a port of `pi-intercom/broker/spawn.ts:179-387`.
//!
//! `ensure_broker` ([`spawnBrokerIfNeeded`]) is idempotent: if the broker is already health-
//! connectable it returns; otherwise it takes an exclusive spawn lock (`O_EXCL`), re-checks, and
//! **re-execs the broker as a detached OS process** — the configured `broker_command`/`broker_args`
//! (`IntercomConfig`, `config.ts:24,26`) if they differ from pi's own `"npx" ["--no-install","tsx"]`
//! default, else `current_exe()` (or the `CYRUP_INTERCOM_BROKER_BINARY` override) with argv
//! `["__intercom-broker"]` — mirroring `getBrokerLaunchSpec`'s `usesDefaultBrokerCommand` gate
//! (`spawn.ts:67-72,121-154`) and `ensureConnected`'s `spawnBrokerIfNeeded(config.brokerCommand,
//! config.brokerArgs)` call (`index.ts:828`). Stdio null, `process_group(0)` (the unsafe-free analog
//! of pi's `detached:true`, mirroring `cyrup-ext-subagents`' `spawn_detached_runner`). The spawned
//! child is then raced against `wait_for_broker` (5 s, `spawn.ts:205-237`): an early exit/error
//! surfaces immediately as a descriptive [`IntercomError::Broker`] instead of waiting out the full
//! timeout. The lock is released in a `finally`-equivalent on every path.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use crate::error::{IntercomError, Result};
use crate::paths;
use crate::transport::framing::{FrameReader, encode_json};
use crate::transport::protocol::{HealthMessage, PROTOCOL_NAME, PROTOCOL_VERSION, now_ms};

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
    let socket_path = paths::broker_socket_path(&intercom_dir);
    let pid_path = paths::broker_pid_path(&intercom_dir);
    let lock_path = paths::broker_spawn_lock_path(&intercom_dir);
    // `ensureConnected` passes `config.brokerCommand`/`config.brokerArgs` straight through to
    // `spawnBrokerIfNeeded` (`index.ts:828`) — load the same config here so a user override
    // genuinely changes what gets launched below, instead of being silently ignored.
    let config = crate::config::load_config(&intercom_dir);

    if is_broker_running(&socket_path, &pid_path).await {
        return Ok(());
    }

    if !acquire_spawn_lock(&lock_path) {
        // Another process is spawning — just wait for it (spawn.ts:187-190).
        return wait_for_broker(&socket_path, WAIT_FOR_BROKER_TIMEOUT).await;
    }

    // Owner path — release the lock on every exit (spawn.ts:238-240).
    let result = spawn_owner(agent_dir, &socket_path, &pid_path, &config.broker_command, &config.broker_args).await;
    release_spawn_lock(&lock_path);
    result
}

async fn spawn_owner(
    agent_dir: &Path,
    socket_path: &Path,
    pid_path: &Path,
    broker_command: &str,
    broker_args: &[String],
) -> Result<()> {
    // Re-check now that we hold the lock (spawn.ts:193-195).
    if is_broker_running(socket_path, pid_path).await {
        return Ok(());
    }
    let mut child = spawn_detached_broker(agent_dir, broker_command, broker_args)?;
    // Race the health-poll against the child's own exit (spawn.ts:205-236): a broker that fails to
    // spawn or dies before startup completes must fail fast with its exit code/signal, not silently
    // wait out the full 5s timeout only to report a generic "timed out".
    let result = tokio::select! {
        res = wait_for_broker(socket_path, WAIT_FOR_BROKER_TIMEOUT) => res,
        wait = child.wait() => Err(broker_wait_error(wait)),
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
) -> Result<tokio::process::Child> {
    let (binary, args) = resolve_broker_command(broker_command, broker_args);
    let mut command = tokio::process::Command::new(&binary);
    command
        .args(&args)
        .env(paths::ENV_CODING_AGENT_DIR, agent_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        command.process_group(0);
    }
    let child = command.spawn().map_err(|e| IntercomError::Broker(format!("failed to spawn intercom broker: {e}")))?;
    Ok(child)
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

/// `isBrokerRunning` (`spawn.ts:243-259`): a health-connectable socket, OR a live pid file
/// (`broker.pid` exists, `kill(pid,0)` succeeds) plus a connectable socket.
pub async fn is_broker_running(socket_path: &Path, pid_path: &Path) -> bool {
    if check_socket_connectable(socket_path).await {
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
    check_socket_connectable(socket_path).await
}

/// `checkSocketConnectable` (`spawn.ts:267-313`): connect, send a `health` probe, and require a
/// byte-identical `health_ok` (`{protocol:"pi-intercom",version:1}`) within 1 s.
pub async fn check_socket_connectable(socket_path: &Path) -> bool {
    let request_id = uuid::Uuid::new_v4().to_string();
    let probe = async {
        let mut stream = UnixStream::connect(socket_path).await.ok()?;
        let frame = encode_json(&HealthMessage::Health {
            request_id: request_id.clone(),
            state_id: None,
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

/// `waitForBroker` (`spawn.ts:378-387`): poll `check_socket_connectable` every 100 ms up to 5 s.
///
/// # Errors
/// [`IntercomError::Broker`] if the broker did not become connectable within `timeout`.
pub async fn wait_for_broker(socket_path: &Path, timeout: Duration) -> Result<()> {
    let start = tokio::time::Instant::now();
    while start.elapsed() < timeout {
        if check_socket_connectable(socket_path).await {
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
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

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
        let socket_path = intercom_dir.join("broker.sock");
        let pid_path = intercom_dir.join("broker.pid");

        let start = std::time::Instant::now();
        // "false" is a non-default broker_command (per `uses_default_broker_command`), so this also
        // exercises the custom-command path landing on a real, immediately-exiting process.
        let result = spawn_owner(dir.path(), &socket_path, &pid_path, "false", &[]).await;
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
