//! Agent-dir / intercom-dir resolution + runtime-dir/file mode restriction — a faithful
//! port of `pi-intercom/broker/paths.ts`, retargeted to cyrup's `~/.cyrup` home (the port doc §7.3;
//! `crates/cyrup-ext-subagents/src/background/mod.rs:1086-1100` resolves the same `<home>/.cyrup`
//! root from `CYRUP_HOME`/`HOME`).
//!
//! What lives here is the part of `paths.ts` that is NOT transport-specific: the
//! cyrup-home/agent-dir/intercom-dir resolution ([`agent_dir_path`], [`intercom_dir_path`]) and the
//! runtime-dir/mode helpers ([`ensure_intercom_runtime_dir`], [`restrict_intercom_runtime_file`],
//! `paths.ts:5-6,118-135`).
//!
//! Everything that answers "where does the broker LIVE" now lives in exactly one module,
//! [`crate::transport::target`] — the per-`<intercomDir>` runtime-file paths
//! ([`broker_port_file_path`](crate::transport::target::broker_port_file_path),
//! [`unix_socket_path`], [`broker_pid_path`], [`broker_spawn_lock_path`]) alongside the
//! platform/transport CHOICE that selects between them: `<intercomDir>/broker.sock` vs the Windows
//! named pipe `\\.\pipe\cyrup-intercom-…` (`getBrokerSocketPath`, `paths.ts:65-74`) vs the opt-in
//! TCP-loopback endpoint (`paths.ts:44-116`, `CYRUP_INTERCOM_TRANSPORT=tcp`), which both the client
//! ([`broker_connect_target`](crate::transport::target::broker_connect_target)) and the broker
//! ([`broker_listen_target`](crate::transport::target::broker_listen_target)) resolve through.
//! All three are bound on the BROKER side as of ICOM-015; the TCP one additionally publishes
//! `<intercomDir>/broker.port.json` (`broker.ts:131-141`).
//!
//! They were previously split across this module (the POSIX socket path plus the pid/lock paths) and
//! `transport::target` (the port-file path and the platform choice), so `getBrokerSocketPath` had two
//! callable spellings at two tree levels and the crate-root, POSIX-only one was the shorter import —
//! which is how the production session connect path came to hard-code the POSIX arm.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// `intercom/` directory mode (`paths.ts:5` `INTERCOM_DIR_MODE = 0o700`).
pub const INTERCOM_DIR_MODE: u32 = 0o700;
/// Runtime-file mode (`paths.ts:6` `INTERCOM_RUNTIME_FILE_MODE = 0o600`).
pub const INTERCOM_RUNTIME_FILE_MODE: u32 = 0o600;

/// The env var overriding the agent directory (`paths.ts:32`, pi `PI_CODING_AGENT_DIR`).
pub const ENV_CODING_AGENT_DIR: &str = cyrup_config::paths::ENV_CODING_AGENT_DIR;

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
    // THE home ladder, shared with every other crate. This used to be spelled out here, and
    // `cyrup_ext_subagents::native_supervisor::intercom_agent_dir_from` carried a byte-identical
    // copy whose doc said it could not import this one "across a dependency edge that forbids
    // importing it". Both now call `cyrup_config::paths`, which sits below both.
    //
    // The ENV rungs only (`cyrup_home_override_from`, not `cyrup_home_dir_from`): `home_dir` is
    // this function's own OS-homedir seam, mirroring pi's `getAgentDirPath(env, homeDir, cwd)`, and
    // it exists so the resolution table is provable without touching process state. The full ladder
    // ends in `ambient_home`, which would answer first and leave that parameter dead — an ambient
    // read smuggled back into a function written to have none.
    let home = cyrup_config::paths::cyrup_home_override_from(&|key| env(key).map(OsString::from))
        .or_else(home_dir)
        .unwrap_or_else(std::env::temp_dir);
    home.join(".cyrup")
}

/// `<agentDir>/intercom` (`getIntercomDirPath`, `paths.ts:40-42`).
#[must_use]
pub fn intercom_dir_path(agent_dir: &Path) -> PathBuf {
    agent_dir.join("intercom")
}

/// `<intercomDir>/extension-state` — the directory `ExtensionStateManager` owns
/// (`pi-intercom/broker/extension-state.ts:54`). One file per namespace, each named by the sha256
/// of that namespace.
#[must_use]
pub fn extension_state_dir_path(intercom_dir: &Path) -> PathBuf {
    intercom_dir.join("extension-state")
}

/// The broker runtime-file paths, re-exported from their single definition site in
/// [`crate::transport::target`] so the call sites that still spell them `paths::…` keep resolving:
/// `paths::broker_pid_path` (`broker/lifecycle.rs`, `transport/spawn.rs`) and
/// `paths::broker_spawn_lock_path` (`transport/spawn.rs`).
///
/// `unix_socket_path` is re-exported under its new, unambiguous name only — see the alias below for
/// why the old `broker_socket_path` spelling still resolves too.
pub use crate::transport::target::{broker_pid_path, broker_spawn_lock_path, unix_socket_path};

/// The former crate-root spelling of [`unix_socket_path`](crate::transport::target::unix_socket_path).
///
/// `#[doc(hidden)]` and deliberately not part of this module's documented surface: the name reads as
/// the general `getBrokerSocketPath` (`paths.ts:65-74`) while resolving only its POSIX arm, which is
/// exactly the confusion that put the wrong endpoint into the production connect path. It survives
/// solely because `crates/cyrup-it/tests/intercom/{reconnect,intercom_id_command,
/// registers_under_session_id,intercom_command_transcript,presence_context_usage}.rs` still import
/// `cyrup_intercom::paths::broker_socket_path`; those are POSIX-only seam tests, so the arm they get
/// is the one they mean. Once they move to `transport::target::unix_socket_path` (or to
/// `broker_connect_target`), delete this alias.
#[doc(hidden)]
pub use crate::transport::target::unix_socket_path as broker_socket_path;

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
        std::fs::set_permissions(
            intercom_dir,
            std::fs::Permissions::from_mode(INTERCOM_DIR_MODE),
        )?;
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
            unix_socket_path(&intercom),
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
        assert!(
            result.is_err(),
            "expected chmod-on-missing-file to surface an Err, not swallow it"
        );
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
