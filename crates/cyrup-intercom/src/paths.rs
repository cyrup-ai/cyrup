//! Agent-dir / intercom-dir / socket-path resolution + runtime-file mode restriction — a faithful
//! port of `pi-intercom/broker/paths.ts`, retargeted to cyrup's `~/.cyrup` home (the port doc §7.3;
//! `crates/cyrup-ext-subagents/src/background/mod.rs:1086-1100` resolves the same `<home>/.cyrup`
//! root from `CYRUP_HOME`/`HOME`).
//!
//! This module is the POSIX arm only: [`broker_socket_path`] is `getBrokerSocketPath`'s
//! `<intercomDir>/broker.sock` branch. The platform CHOICE — that same branch vs the Windows named
//! pipe `\\.\pipe\cyrup-intercom-…` (`paths.ts:65-74`), and the opt-in TCP-loopback endpoint
//! (`paths.ts:44-116`, `CYRUP_INTERCOM_TRANSPORT=tcp`) — lives in
//! [`crate::transport::target`], which both the client
//! ([`broker_connect_target`](crate::transport::target::broker_connect_target)) and the broker
//! ([`broker_listen_target`](crate::transport::target::broker_listen_target)) resolve through.
//! Of those three transports only the opt-in TCP one is still unported on the
//! BROKER side; see [`crate::broker::listener::BrokerListener::bind`] and the port doc §10-Q2.

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
/// pi uses `<home>/.pi/agent`, with `<home>` resolved from pi's `homeDir: string = homedir()`
/// default parameter (Node's `os.homedir()`, which on POSIX checks `$HOME` and, failing that,
/// consults the system password database for the real user's home directory — total whenever the
/// OS has ANY notion of a home dir for the running user).
///
/// cyrup uses `<home>/.cyrup` (the port doc §7.3), with `<home>` resolved the same way: `CYRUP_HOME`
/// first (a cyrup-only additive override with no pi counterpart, mirroring `subagents_home`,
/// `cyrup-ext-subagents/src/background/mod.rs:1096-1100`), then `HOME`, then — mirroring
/// `os.homedir()`'s passwd-db fallback — `std::env::home_dir()` (checks `$HOME` then falls back to
/// the platform's real-user-home lookup), and only as a truly last resort (no home dir resolvable
/// at all) the OS temp dir, so this remains total.
#[must_use]
pub fn agent_dir_path() -> PathBuf {
    agent_dir_path_from(
        |k| std::env::var(k).ok(),
        std::env::home_dir,
        std::env::current_dir().ok(),
    )
}

/// The pure core of [`agent_dir_path`], parameterized over the env lookup, the OS-homedir lookup,
/// and cwd (mirroring pi's own `getAgentDirPath(env, homeDir, cwd)` three-parameter signature,
/// `paths.ts:27-31`) so the resolution table can be unit-tested without mutating process-global env
/// state (`set_var` is `unsafe` under edition 2024) or depending on the real OS user database.
#[must_use]
pub fn agent_dir_path_from(
    env: impl Fn(&str) -> Option<String>,
    home_dir: impl Fn() -> Option<PathBuf>,
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
        .or_else(home_dir)
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

/// `restrictIntercomRuntimeFile` (`paths.ts:128-135`): `chmod 0o600` on non-Windows.
///
/// pi's `chmodSync` call has no try/catch — any failure (ENOENT, EPERM, ...) throws and propagates
/// uncaught to the caller (the broker.ts call sites never wrap it; the one caller that does,
/// spawn.ts:317-327, explicitly re-throws unless the error is `EEXIST`). A chmod failure is never
/// silently discarded anywhere in pi, so this must propagate rather than swallow.
///
/// # Errors
/// Propagates any `set_permissions` I/O failure (e.g. the target file does not exist, or the
/// caller lacks permission to change its mode).
pub fn restrict_intercom_runtime_file(file_path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            file_path,
            std::fs::Permissions::from_mode(INTERCOM_RUNTIME_FILE_MODE),
        )?;
    }
    #[cfg(not(unix))]
    {
        let _ = file_path;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn agent_dir_prefers_absolute_override_verbatim() {
        let dir = agent_dir_path_from(
            |k| (k == ENV_CODING_AGENT_DIR).then(|| "/abs/agent".to_string()),
            || None,
            Some(PathBuf::from("/cwd")),
        );
        assert_eq!(dir, PathBuf::from("/abs/agent"));
    }

    #[test]
    fn agent_dir_resolves_relative_override_against_cwd() {
        let dir = agent_dir_path_from(
            |k| (k == ENV_CODING_AGENT_DIR).then(|| "rel/agent".to_string()),
            || None,
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
            || None,
            Some(PathBuf::from("/cwd")),
        );
        assert_eq!(dir, PathBuf::from("/home/me/.cyrup"));
    }

    /// Regression proof for the home-directory-fallback divergence: pi's `homedir()` default
    /// (`os.homedir()`) always resolves the OS-known real home directory via a passwd-db-style
    /// fallback when `$HOME` is unset. Before this fix, cyrup fell straight to the OS temp dir in
    /// that case (`unwrap_or_else(std::env::temp_dir)` with no intermediate `home_dir` fallback) —
    /// this test would have failed against that prior behavior (it would have produced a path under
    /// `std::env::temp_dir()` instead of the OS-resolved home).
    #[test]
    fn agent_dir_falls_back_to_os_home_dir_when_no_env_override_present() {
        let dir = agent_dir_path_from(
            |_| None,
            || Some(PathBuf::from("/real/os/home")),
            Some(PathBuf::from("/cwd")),
        );
        assert_eq!(dir, PathBuf::from("/real/os/home/.cyrup"));
    }

    /// `CYRUP_HOME` and `HOME` both take priority over the `home_dir` OS fallback (order-of-
    /// operations parity: pi's `homedir()` is only ever consulted as the `homeDir` default when no
    /// override plays a role upstream of it; cyrup's additive `CYRUP_HOME`/`HOME` checks must still
    /// run first).
    #[test]
    fn agent_dir_env_home_overrides_take_priority_over_os_home_dir() {
        let dir = agent_dir_path_from(
            |k| (k == "HOME").then(|| "/env/home".to_string()),
            || Some(PathBuf::from("/real/os/home")),
            Some(PathBuf::from("/cwd")),
        );
        assert_eq!(dir, PathBuf::from("/env/home/.cyrup"));
    }

    /// The OS temp dir remains the absolute last resort only when even the `home_dir` fallback is
    /// unresolvable (mirroring the "total" guarantee documented on `agent_dir_path`).
    #[test]
    fn agent_dir_falls_back_to_temp_dir_only_when_home_dir_also_unresolvable() {
        let dir = agent_dir_path_from(|_| None, || None, Some(PathBuf::from("/cwd")));
        assert_eq!(dir, std::env::temp_dir().join(".cyrup"));
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

    /// Regression proof for the swallowed-chmod-error divergence: pi's `chmodSync` has no
    /// try/catch, so any failure (ENOENT etc.) propagates uncaught to the caller. Before this fix,
    /// `restrict_intercom_runtime_file` returned `()` and discarded the `set_permissions` result via
    /// `let _ = ...`, so this test — which chmods a path that cannot possibly exist — would have
    /// passed silently with no way to observe the failure; now it must surface as `Err`.
    #[test]
    fn restrict_runtime_file_propagates_set_permissions_failure() {
        let missing = PathBuf::from("/nonexistent/does/not/exist/broker.sock");
        let result = restrict_intercom_runtime_file(&missing);
        assert!(result.is_err(), "expected chmod-on-missing-file to surface an Err, not swallow it");
    }

    #[test]
    fn restrict_runtime_file_succeeds_on_real_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("broker.sock");
        std::fs::write(&file_path, b"").unwrap();
        let result = restrict_intercom_runtime_file(&file_path);
        assert!(result.is_ok());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&file_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, INTERCOM_RUNTIME_FILE_MODE);
        }
    }
}
