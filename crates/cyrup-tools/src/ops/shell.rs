//! Shell detection & command transport (R-03-025, arch-03 §6.9; Pi `utils/shell.ts`).
//!
//! `ShellConfig` records the shell program, its args, and the command-transport style (argv vs
//! stdin). Resolution mirrors Pi's `getShellConfig` (shell.ts:20-120): an explicit settings shell
//! path first (with the `Custom shell path not found` error), then `/bin/bash -c` on unix, then a
//! `which bash` PATH fallback, then `sh -c`. A WSL-legacy `…\Windows\System32\bash.exe` is driven
//! over **stdin** (`bash -s`) rather than argv (shell.ts:15-22).

use cyrup_core::ToolError;
use std::path::{Path, PathBuf};

/// How the command text reaches the shell (R-03-025 dual transport).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    /// `shell <args...> "command"` (e.g. `bash -c "cmd"`).
    Argv,
    /// Pipe the command via stdin to a shell started with no command arg (`bash -s` / `sh`).
    Stdin,
}

/// A resolved shell configuration.
#[derive(Clone, Debug)]
pub struct ShellConfig {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub transport: Transport,
}

/// `isLegacyWslBashPath` (shell.ts:15-18): a Windows-namespaced `…\Windows\{System32,Sysnative}\
/// bash.exe` (case-insensitive, slashes normalized) is the legacy WSL launcher, which only accepts
/// the command over stdin.
fn is_legacy_wsl_bash_path(path: &str) -> bool {
    let normalized = path.replace('/', "\\").to_ascii_lowercase();
    let rest = match normalized.split_once(':') {
        Some((drive, rest))
            if drive.chars().count() == 1
                && drive.chars().next().is_some_and(|c| c.is_ascii_lowercase()) =>
        {
            rest
        }
        _ => return false,
    };
    rest == "\\windows\\system32\\bash.exe" || rest == "\\windows\\sysnative\\bash.exe"
}

/// `getBashShellConfig` (shell.ts:20-22): stdin transport for the WSL-legacy launcher, argv `-c`
/// otherwise.
fn get_bash_shell_config(program: PathBuf) -> ShellConfig {
    if is_legacy_wsl_bash_path(&program.to_string_lossy()) {
        ShellConfig { program, args: vec!["-s".to_string()], transport: Transport::Stdin }
    } else {
        ShellConfig { program, args: vec!["-c".to_string()], transport: Transport::Argv }
    }
}

/// `findBashOnPath` (shell.ts:24-58): `which bash` on unix / `where bash.exe` on Windows. Returns
/// the first match (verified to exist on Windows, where `where` can print stale paths).
fn find_bash_on_path() -> Option<PathBuf> {
    #[cfg(not(unix))]
    let (cmd, arg) = ("where", "bash.exe");
    #[cfg(unix)]
    let (cmd, arg) = ("which", "bash");

    let output = std::process::Command::new(cmd).arg(arg).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let first = stdout.lines().map(str::trim).find(|l| !l.is_empty())?;
    let path = PathBuf::from(first);
    #[cfg(not(unix))]
    {
        if !path.exists() {
            return None;
        }
    }
    Some(path)
}

impl ShellConfig {
    /// Build from an explicit program with `-c` argv transport (or stdin for the WSL-legacy path).
    pub fn argv(program: impl Into<PathBuf>) -> Self {
        get_bash_shell_config(program.into())
    }

    /// Resolve the shell, honoring an explicit settings `shellPath` (Pi `getShellConfig`,
    /// shell.ts:67-120). A provided-but-missing path is the only error case (`Custom shell path
    /// not found: …`), surfaced per-exec exactly like Pi's `createLocalBashOperations`.
    pub fn resolve(custom_shell_path: Option<&str>) -> Result<Self, ToolError> {
        if let Some(p) = custom_shell_path {
            if Path::new(p).exists() {
                return Ok(get_bash_shell_config(PathBuf::from(p)));
            }
            return Err(ToolError::new(format!("Custom shell path not found: {p}")));
        }
        Ok(Self::detect())
    }

    /// Detect the platform default shell (R-03-025), mirroring Pi's no-`shellPath` branch.
    pub fn detect() -> Self {
        // `CYRUP_SHELL` is a cyrup-specific override (honored through `get_bash_shell_config` so a
        // WSL-legacy override still selects stdin transport); it has no Pi analogue.
        if let Some(explicit) = std::env::var_os("CYRUP_SHELL") {
            return get_bash_shell_config(PathBuf::from(explicit));
        }
        #[cfg(unix)]
        {
            // Pi unix order (shell.ts:109-119): `/bin/bash`, then `which bash`, then `sh -c`.
            if Path::new("/bin/bash").exists() {
                return get_bash_shell_config(PathBuf::from("/bin/bash"));
            }
            if let Some(found) = find_bash_on_path() {
                return get_bash_shell_config(found);
            }
            ShellConfig {
                program: PathBuf::from("sh"),
                args: vec!["-c".to_string()],
                transport: Transport::Argv,
            }
        }
        #[cfg(not(unix))]
        {
            // Pi Windows order (shell.ts:76-106): Git Bash in known locations, then `where bash.exe`,
            // then (cyrup pragmatic fallback) cmd.exe `/C`.
            let mut candidates: Vec<PathBuf> = Vec::new();
            if let Some(pf) = std::env::var_os("ProgramFiles") {
                candidates.push(PathBuf::from(pf).join("Git").join("bin").join("bash.exe"));
            }
            if let Some(pf86) = std::env::var_os("ProgramFiles(x86)") {
                candidates.push(PathBuf::from(pf86).join("Git").join("bin").join("bash.exe"));
            }
            for cand in candidates {
                if cand.exists() {
                    return get_bash_shell_config(cand);
                }
            }
            if let Some(found) = find_bash_on_path() {
                return get_bash_shell_config(found);
            }
            ShellConfig {
                program: PathBuf::from("cmd.exe"),
                args: vec!["/C".to_string()],
                transport: Transport::Argv,
            }
        }
    }
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self::detect()
    }
}

/// Build the child-process env OVERRIDES, prepending a managed `bin_dir` to `PATH` (Pi
/// `getShellEnv`, shell.ts:122-134). Returns an empty vec when `bin_dir` is `None` (inherit the
/// parent env unchanged). The `PATH` key is matched case-insensitively (Windows `Path`); the bin
/// dir is prepended only if not already present.
pub fn shell_env(bin_dir: Option<&Path>) -> Vec<(String, String)> {
    let Some(bin_dir) = bin_dir else {
        return Vec::new();
    };
    let bin = bin_dir.to_string_lossy().into_owned();
    if bin.is_empty() {
        return Vec::new();
    }
    let delimiter = if cfg!(windows) { ';' } else { ':' };
    let (key, current) = std::env::vars_os()
        .find(|(k, _)| k.to_string_lossy().eq_ignore_ascii_case("path"))
        .map(|(k, v)| (k.to_string_lossy().into_owned(), v.to_string_lossy().into_owned()))
        .unwrap_or_else(|| ("PATH".to_string(), String::new()));
    let already = current.split(delimiter).any(|e| !e.is_empty() && e == bin);
    let updated = if already {
        current
    } else if current.is_empty() {
        bin
    } else {
        format!("{bin}{delimiter}{current}")
    };
    vec![(key, updated)]
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn wsl_legacy_path_uses_stdin_transport() {
        let cfg = ShellConfig::argv(r"C:\Windows\System32\bash.exe");
        assert_eq!(cfg.transport, Transport::Stdin);
        assert_eq!(cfg.args, vec!["-s".to_string()]);
        // Forward slashes + mixed case normalize the same way.
        assert!(is_legacy_wsl_bash_path("c:/windows/sysnative/bash.exe"));
        // A normal bash path is argv.
        assert!(!is_legacy_wsl_bash_path("/bin/bash"));
        assert_eq!(ShellConfig::argv("/bin/bash").transport, Transport::Argv);
    }

    #[test]
    fn resolve_missing_custom_path_errors() {
        let err = ShellConfig::resolve(Some("/no/such/shell/here")).unwrap_err();
        assert!(err.to_string().contains("Custom shell path not found"));
    }

    #[test]
    fn resolve_existing_custom_path_ok() {
        // A path that exists on every unix CI box.
        #[cfg(unix)]
        {
            let cfg = ShellConfig::resolve(Some("/bin/sh")).unwrap();
            assert_eq!(cfg.program, PathBuf::from("/bin/sh"));
            assert_eq!(cfg.transport, Transport::Argv);
        }
    }

    #[test]
    fn shell_env_none_inherits() {
        assert!(shell_env(None).is_empty());
    }

    #[test]
    fn shell_env_prepends_bin_dir() {
        let env = shell_env(Some(Path::new("/opt/cyrup/bin")));
        assert_eq!(env.len(), 1);
        let (k, v) = &env[0];
        assert!(k.eq_ignore_ascii_case("path"));
        assert!(v.starts_with("/opt/cyrup/bin"));
    }
}
