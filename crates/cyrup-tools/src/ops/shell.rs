//! Shell detection & command transport (R-03-025, arch-03 §6.9).
//!
//! `ShellConfig` records the shell program, its args, and the command-transport style (argv vs
//! stdin). Resolution prefers an explicit shell path, then `/bin/bash -c` on unix, then `sh -c`.
//! WSL-legacy / no-args shells use stdin transport.

use std::path::PathBuf;

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

impl ShellConfig {
    /// Build from an explicit program with `-c` argv transport.
    pub fn argv(program: impl Into<PathBuf>) -> Self {
        Self { program: program.into(), args: vec!["-c".to_string()], transport: Transport::Argv }
    }

    /// Detect the platform default shell (R-03-025).
    pub fn detect() -> Self {
        if let Some(explicit) = std::env::var_os("CYRUP_SHELL") {
            return ShellConfig::argv(PathBuf::from(explicit));
        }
        #[cfg(unix)]
        {
            for candidate in ["/bin/bash", "/usr/bin/bash", "/bin/sh"] {
                if std::path::Path::new(candidate).exists() {
                    return ShellConfig::argv(candidate);
                }
            }
            ShellConfig::argv("/bin/sh")
        }
        #[cfg(not(unix))]
        {
            // Windows fallback: cmd.exe /C. Argv transport.
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
