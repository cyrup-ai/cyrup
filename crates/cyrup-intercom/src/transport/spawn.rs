//! Broker discovery + auto-spawn — a port of `pi-intercom/broker/spawn.ts:179-387`.
//!
//! `ensure_broker` ([`spawnBrokerIfNeeded`]) is idempotent: if the broker is already health-
//! connectable it returns; otherwise it takes an exclusive spawn lock (`O_EXCL`), re-checks, and
//! **re-execs the broker as a detached OS process** — `current_exe()` (or the
//! `CYRUP_INTERCOM_BROKER_BINARY` override) with argv `["__intercom-broker"]`, stdio null,
//! `process_group(0)` (the unsafe-free analog of pi's `detached:true`, mirroring
//! `cyrup-ext-subagents`' `spawn_detached_runner`), then polls `wait_for_broker` (5 s). The lock is
//! released in a `finally`-equivalent on every path.

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

    if is_broker_running(&socket_path, &pid_path).await {
        return Ok(());
    }

    if !acquire_spawn_lock(&lock_path) {
        // Another process is spawning — just wait for it (spawn.ts:187-190).
        return wait_for_broker(&socket_path, WAIT_FOR_BROKER_TIMEOUT).await;
    }

    // Owner path — release the lock on every exit (spawn.ts:238-240).
    let result = spawn_owner(agent_dir, &socket_path, &pid_path).await;
    release_spawn_lock(&lock_path);
    result
}

async fn spawn_owner(agent_dir: &Path, socket_path: &Path, pid_path: &Path) -> Result<()> {
    // Re-check now that we hold the lock (spawn.ts:193-195).
    if is_broker_running(socket_path, pid_path).await {
        return Ok(());
    }
    spawn_detached_broker(agent_dir)?;
    wait_for_broker(socket_path, WAIT_FOR_BROKER_TIMEOUT).await
}

/// Resolve which binary to re-exec + its argv (`getBrokerLaunchSpec`, `spawn.ts:121-154`, cyrup
/// re-exec form): the `CYRUP_INTERCOM_BROKER_BINARY` override else `current_exe()` else the literal
/// `"cyrup"`, always followed by the `__intercom-broker` subcommand.
#[must_use]
pub fn resolve_broker_command() -> (PathBuf, Vec<String>) {
    if let Ok(bin) = std::env::var(ENV_INTERCOM_BROKER_BINARY)
        && !bin.trim().is_empty()
    {
        return (PathBuf::from(bin), vec![INTERCOM_BROKER_SUBCOMMAND.to_string()]);
    }
    let binary = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("cyrup"));
    (binary, vec![INTERCOM_BROKER_SUBCOMMAND.to_string()])
}

/// Spawn the broker as a genuinely detached process (`spawn.ts:202-203`, cyrup form): stdio null,
/// `process_group(0)`, env overlay `CYRUP_CODING_AGENT_DIR=<abs agent dir>`, child handle dropped
/// without awaiting. Mirrors `cyrup-ext-subagents::background::spawn_detached_runner`.
fn spawn_detached_broker(agent_dir: &Path) -> Result<()> {
    let (binary, args) = resolve_broker_command();
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
    // THE POINT: drop the child handle without ever awaiting it — the broker keeps running under its
    // own detached process group (mirrors spawn_detached_runner's `drop(child)`).
    drop(child);
    Ok(())
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
                paths::restrict_intercom_runtime_file(lock_path);
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
        let (_bin, args) = resolve_broker_command();
        assert_eq!(args, vec![INTERCOM_BROKER_SUBCOMMAND.to_string()]);
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
