//! Agent-dir / intercom-dir / socket-path resolution + runtime-file mode restriction — a faithful
//! port of `pi-intercom/broker/paths.ts`, retargeted to cyrup's `~/.cyrup` home (the port doc §7.3;
//! `crates/cyrup-ext-subagents/src/background/mod.rs:1086-1100` resolves the same `<home>/.cyrup`
//! root from `CYRUP_HOME`/`HOME`).
//!
//! First cyrup milestone: **Unix domain socket only** (macOS/Linux). The Windows named-pipe and
//! opt-in TCP-loopback transports (`paths.ts:44-116`) are deferred behind the same env gates
//! (`CYRUP_INTERCOM_TRANSPORT=tcp`) — see the port doc §10-Q2.

use std::path::{Path, PathBuf};

/// `intercom/` directory mode (`paths.ts:5` `INTERCOM_DIR_MODE = 0o700`).
pub const INTERCOM_DIR_MODE: u32 = 0o700;
/// Runtime-file mode (`paths.ts:6` `INTERCOM_RUNTIME_FILE_MODE = 0o600`).
pub const INTERCOM_RUNTIME_FILE_MODE: u32 = 0o600;

/// The env var overriding the agent directory (`paths.ts:32`, pi `PI_CODING_AGENT_DIR`).
pub const ENV_CODING_AGENT_DIR: &str = "CYRUP_CODING_AGENT_DIR";

/// Resolve the agent directory (`getAgentDirPath`, `paths.ts:27-38`): `$CYRUP_CODING_AGENT_DIR`
/// (absolute verbatim, else resolved against `cwd`) if set and non-blank, else `<home>/.cyrup`.
///
/// pi uses `<home>/.pi/agent`; cyrup uses `<home>/.cyrup` (the port doc §7.3). `<home>` resolves
/// from `CYRUP_HOME`, then `HOME` (mirroring `subagents_home`,
/// `cyrup-ext-subagents/src/background/mod.rs:1096-1100`), then the OS temp dir as a last resort so
/// this is total.
#[must_use]
pub fn agent_dir_path() -> PathBuf {
    agent_dir_path_from(|k| std::env::var(k).ok(), std::env::current_dir().ok())
}

/// The pure core of [`agent_dir_path`], parameterized over the env lookup and cwd so the resolution
/// table can be unit-tested without mutating process-global env state (`set_var` is `unsafe` under
/// edition 2024).
#[must_use]
pub fn agent_dir_path_from(
    env: impl Fn(&str) -> Option<String>,
    cwd: Option<PathBuf>,
) -> PathBuf {
    if let Some(configured) = env(ENV_CODING_AGENT_DIR)
        && !configured.trim().is_empty()
    {
        let configured = configured.trim();
        let p = Path::new(configured);
        return if p.is_absolute() {
            p.to_path_buf()
        } else {
            match &cwd {
                Some(base) => base.join(p),
                None => p.to_path_buf(),
            }
        };
    }
    let home = env("CYRUP_HOME")
        .map(PathBuf::from)
        .or_else(|| env("HOME").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir);
    home.join(".cyrup")
}

/// `<agentDir>/intercom` (`getIntercomDirPath`, `paths.ts:40-42`).
#[must_use]
pub fn intercom_dir_path(agent_dir: &Path) -> PathBuf {
    agent_dir.join("intercom")
}

/// `<intercomDir>/broker.sock` (`getBrokerSocketPath`, `paths.ts:65-74`; Unix branch only).
#[must_use]
pub fn broker_socket_path(intercom_dir: &Path) -> PathBuf {
    intercom_dir.join("broker.sock")
}

/// `<intercomDir>/broker.pid` (`broker.ts:22`).
#[must_use]
pub fn broker_pid_path(intercom_dir: &Path) -> PathBuf {
    intercom_dir.join("broker.pid")
}

/// `<intercomDir>/broker.spawn.lock` (`spawn.ts:24`).
#[must_use]
pub fn broker_spawn_lock_path(intercom_dir: &Path) -> PathBuf {
    intercom_dir.join("broker.spawn.lock")
}

/// `ensureIntercomRuntimeDir` (`paths.ts:118-126`): `mkdir -p` at mode `0o700`, then re-`chmod`
/// on non-Windows (a pre-existing dir keeps whatever mode it had until this re-chmods it).
///
/// # Errors
/// Propagates any `create_dir_all`/`set_permissions` I/O failure.
pub fn ensure_intercom_runtime_dir(intercom_dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(intercom_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(intercom_dir, std::fs::Permissions::from_mode(INTERCOM_DIR_MODE))?;
    }
    Ok(())
}

/// `restrictIntercomRuntimeFile` (`paths.ts:128-135`): `chmod 0o600` on non-Windows. Best-effort
/// (a missing file / EPERM is not fatal to the caller's own success path).
pub fn restrict_intercom_runtime_file(file_path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(
            file_path,
            std::fs::Permissions::from_mode(INTERCOM_RUNTIME_FILE_MODE),
        );
    }
    #[cfg(not(unix))]
    {
        let _ = file_path;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn agent_dir_prefers_absolute_override_verbatim() {
        let dir = agent_dir_path_from(
            |k| (k == ENV_CODING_AGENT_DIR).then(|| "/abs/agent".to_string()),
            Some(PathBuf::from("/cwd")),
        );
        assert_eq!(dir, PathBuf::from("/abs/agent"));
    }

    #[test]
    fn agent_dir_resolves_relative_override_against_cwd() {
        let dir = agent_dir_path_from(
            |k| (k == ENV_CODING_AGENT_DIR).then(|| "rel/agent".to_string()),
            Some(PathBuf::from("/cwd")),
        );
        assert_eq!(dir, PathBuf::from("/cwd/rel/agent"));
    }

    #[test]
    fn agent_dir_blank_override_falls_through_to_home_dot_cyrup() {
        let dir = agent_dir_path_from(
            |k| match k {
                ENV_CODING_AGENT_DIR => Some("   ".to_string()),
                "HOME" => Some("/home/me".to_string()),
                _ => None,
            },
            Some(PathBuf::from("/cwd")),
        );
        assert_eq!(dir, PathBuf::from("/home/me/.cyrup"));
    }

    #[test]
    fn socket_path_is_intercom_dir_broker_sock() {
        let agent = PathBuf::from("/home/me/.cyrup");
        let intercom = intercom_dir_path(&agent);
        assert_eq!(intercom, PathBuf::from("/home/me/.cyrup/intercom"));
        assert_eq!(
            broker_socket_path(&intercom),
            PathBuf::from("/home/me/.cyrup/intercom/broker.sock")
        );
    }
}
