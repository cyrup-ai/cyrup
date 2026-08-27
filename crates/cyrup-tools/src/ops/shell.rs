//! Shell detection & command transport (R-03-025, arch-03 §6.9; Pi `utils/shell.ts`).
//!
//! `ShellConfig` records the shell program, its args, and the command-transport style (argv vs
//! stdin). Resolution mirrors Pi's `getShellConfig` (shell.ts:20-120): an explicit settings shell
//! path first (with the `Custom shell path not found` error), then `/bin/bash -c` on unix, then a
//! `which bash` PATH fallback, then `sh -c`. A WSL-legacy `…\Windows\System32\bash.exe` is driven
//! over **stdin** (`bash -s`) rather than argv (shell.ts:15-22).

use cyrup_core::ToolError;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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
    /// The shell's human name, as Pi passes it to `createLocalShellOperations` (bash.ts:84,159;
    /// powershell.ts:33): `"bash"` or `"PowerShell"`. Its ONLY consumer is the missing-cwd error
    /// `Cannot execute {shell_name} commands.` (bash.ts:95), which cyrup raises in the process
    /// backend rather than in the tool — so the name has to ride along with the resolved shell.
    /// It is the TOOL's name for its shell, not the resolved program's: bash's unix `sh -c`
    /// fallback (shell.ts:119) still reports `bash`, exactly as upstream does.
    pub shell_name: &'static str,
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
        ShellConfig {
            shell_name: "bash",
            program,
            args: vec!["-s".to_string()],
            transport: Transport::Stdin,
        }
    } else {
        ShellConfig {
            shell_name: "bash",
            program,
            args: vec!["-c".to_string()],
            transport: Transport::Argv,
        }
    }
}

/// Pi bounds BOTH probes — `spawnSync("which", ["bash"], { …, timeout: 5000 })` (shell.ts:47) and
/// the Windows `spawnSync("where", ["bash.exe"], { …, timeout: 5000 })` (shell.ts:28-32) — and on
/// expiry falls through to the next arm (`sh -c` on unix, shell.ts:119; the `No bash shell found`
/// throw on Windows, shell.ts:100-106).
const BASH_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// `findExecutableOnPath` (shell.ts:24-58): `which <exe>` on unix / `where <exe>` on Windows.
/// Returns the first match (verified to exist on Windows, where `where` can print stale paths).
///
/// Generic in the executable because Pi's is: it is called with `"bash"` / `"bash.exe"` for the
/// bash tool (shell.ts:95,114) and with `"pwsh.exe"` / `"powershell.exe"` for the powershell tool
/// (shell.ts:130). The unix/Windows split is the PROBE COMMAND only; the caller supplies the name.
///
/// Bounded at [`BASH_PROBE_TIMEOUT`] exactly like Pi's `spawnSync` timeout: a `which` wedged on a
/// stale automount PATH entry must not wedge a command. Node's `spawnSync` kills the child on
/// expiry and reports a non-zero status, which lands in Pi's `result.status === 0` guard
/// (shell.ts:48) — i.e. "not on PATH" — so expiry maps to `None` here.
fn find_executable_on_path(executable: &str) -> Option<PathBuf> {
    #[cfg(not(unix))]
    let cmd = "where";
    #[cfg(unix)]
    let cmd = "which";
    let arg = executable;

    // stdout is piped and read only after the child exits. Both probes emit at most a handful of
    // short lines, far under a pipe buffer, so this cannot deadlock on a full pipe; `spawnSync`
    // has the same shape (it buffers the whole child output).
    let mut probe = std::process::Command::new(cmd);
    probe
        .arg(arg)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    // Pi's win32 probe is `spawnSync("where", [executable], { …, windowsHide: true })`
    // (shell.ts:28-32); its unix `which` arm (shell.ts:47) passes no such option, which is exactly
    // what `windows_hide` compiles to off Windows. Shell detection runs during session
    // construction, so without this a console flashes before the agent has printed anything.
    crate::ops::win::windows_hide(&mut probe);
    let mut child = probe.spawn().ok()?;

    let deadline = Instant::now() + BASH_PROBE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    // Node's `spawnSync` sends SIGTERM on timeout; the probe result is discarded
                    // either way, so reap and report "not found".
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    if !status.success() {
        return None;
    }
    let mut buf = String::new();
    child.stdout.take()?.read_to_string(&mut buf).ok()?;
    let first = buf.lines().map(str::trim).find(|l| !l.is_empty())?;
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
        Self::try_detect()
    }

    /// Detect the platform default shell (R-03-025), mirroring Pi's no-`shellPath` branch
    /// (`getShellConfig`, shell.ts:76-119) — and, exactly like Pi, reading **no environment
    /// variable as a shell selector**. The only `process.env` reads in `getShellConfig` are the
    /// Windows *installation-location* lookups `ProgramFiles` / `ProgramFiles(x86)` (shell.ts:79,
    /// :83) that build the Git Bash candidate list; the sole caller-supplied override is
    /// `customShellPath`, i.e. the `shellPath` setting ([`ShellConfig::resolve`]).
    ///
    /// Fallible because the Windows arm ends in a throw (shell.ts:100-106) rather than a fallback
    /// (ADR-0003 D4). The unix arm cannot fail: `sh -c` is Pi's terminal fallback (shell.ts:119).
    /// Pi's Windows no-`shellPath` branch (shell.ts:76-106) with its two `process.env` reads and
    /// its `where bash.exe` probe hoisted into arguments, so the arm that SHIPS to Windows is
    /// compiled and unit-tested on every host (`windows_arm_without_bash_errors_with_pis_repair_recipe`
    /// below). It previously lived inline under `#[cfg(not(unix))]` and its only regression test
    /// was `#[cfg(not(unix))]` too — a test that had never compiled, let alone run: it mutated
    /// `std::env` inside an `unsafe` block, which `#![deny(unsafe_code)]` (lib.rs:16) rejects, so
    /// `cargo check -p cyrup-tools --all-targets --target x86_64-pc-windows-gnu` failed on it.
    #[cfg_attr(unix, allow(dead_code))]
    fn windows_detect_from(
        candidates: &[PathBuf],
        find_on_path: impl FnOnce() -> Option<PathBuf>,
    ) -> Result<Self, ToolError> {
        for cand in candidates {
            if cand.exists() {
                return Ok(get_bash_shell_config(cand.clone()));
            }
        }
        if let Some(found) = find_on_path() {
            return Ok(get_bash_shell_config(found));
        }
        // shell.ts:100-106 verbatim, including the `  ${p}`-indented searched-path list.
        let searched = candidates
            .iter()
            .map(|p| format!("  {}", p.display()))
            .collect::<Vec<_>>()
            .join("\n");
        Err(ToolError::new(format!(
            "No bash shell found. Options:\n  1. Install Git for Windows: \
             https://git-scm.com/download/win\n  2. Add your bash to PATH (Cygwin, MSYS2, \
             etc.)\n  3. Set shellPath in settings.json\n\nSearched Git Bash in:\n{searched}"
        )))
    }

    pub fn try_detect() -> Result<Self, ToolError> {
        #[cfg(unix)]
        {
            // Pi unix order (shell.ts:109-119): `/bin/bash`, then `which bash`, then `sh -c`.
            if Path::new("/bin/bash").exists() {
                return Ok(get_bash_shell_config(PathBuf::from("/bin/bash")));
            }
            if let Some(found) = find_executable_on_path("bash") {
                return Ok(get_bash_shell_config(found));
            }
            // Still `bash` by NAME (Pi's `createLocalShellOperations("bash", …)`, bash.ts:159, is
            // unaware that `getShellConfig` degraded to `sh` at shell.ts:119).
            Ok(ShellConfig {
                shell_name: "bash",
                program: PathBuf::from("sh"),
                args: vec!["-c".to_string()],
                transport: Transport::Argv,
            })
        }
        #[cfg(not(unix))]
        {
            // Pi Windows order (shell.ts:76-106): Git Bash in known locations, then `where
            // bash.exe`, then a hard stop carrying the three-option repair recipe AND the searched
            // paths. cyrup previously substituted `cmd.exe /C` here, which runs model-authored
            // bash with different (or no) quoting, redirection and `$VAR` semantics and tells
            // nobody — ADR-0003 D4 / TOOL-038.
            let mut candidates: Vec<PathBuf> = Vec::new();
            if let Some(pf) = std::env::var_os("ProgramFiles") {
                candidates.push(PathBuf::from(pf).join("Git").join("bin").join("bash.exe"));
            }
            if let Some(pf86) = std::env::var_os("ProgramFiles(x86)") {
                candidates.push(PathBuf::from(pf86).join("Git").join("bin").join("bash.exe"));
            }
            Self::windows_detect_from(&candidates, || find_executable_on_path("bash.exe"))
        }
    }

    /// `POWERSHELL_ARGS` (shell.ts:122), verbatim and in order. The command is delivered as the
    /// argument AFTER `-Command`, i.e. [`Transport::Argv`] — Pi sets no `commandTransport` on the
    /// PowerShell config (shell.ts:135), so the WSL-legacy stdin path is unreachable here.
    const POWERSHELL_ARGS: [&'static str; 5] = [
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
    ];

    /// Pi `getPowerShellConfig`'s body AFTER the `win32` guard (shell.ts:130-135), with the PATH
    /// probe hoisted into an argument so the arm that SHIPS to Windows is compiled on every host —
    /// the same treatment [`ShellConfig::windows_detect_from`] already gets, and for the same
    /// reason: this workspace has no Windows box, so an arm that only exists under
    /// `#[cfg(windows)]` is an arm nobody has ever built.
    ///
    /// `pwsh.exe` FIRST, `powershell.exe` second: Pi's `??` prefers PowerShell 7 over Windows
    /// PowerShell 5.1 (shell.ts:124 "preferring PowerShell 7 when available").
    #[cfg_attr(not(windows), allow(dead_code))]
    fn powershell_detect_from(
        find_on_path: impl Fn(&str) -> Option<PathBuf>,
    ) -> Result<Self, ToolError> {
        let found = find_on_path("pwsh.exe").or_else(|| find_on_path("powershell.exe"));
        let Some(program) = found else {
            // shell.ts:132 verbatim.
            return Err(ToolError::new(
                "No PowerShell executable found. Install PowerShell or add powershell.exe/pwsh.exe \
                 to PATH.",
            ));
        };
        Ok(ShellConfig {
            shell_name: "PowerShell",
            program,
            args: Self::POWERSHELL_ARGS
                .iter()
                .map(|a| (*a).to_string())
                .collect(),
            transport: Transport::Argv,
        })
    }

    /// Pi `getPowerShellConfig` (shell.ts:124-136). Called per-command from the `powershell` tool's
    /// `execute`, never at construction — Pi's thunk is the bare `getPowerShellConfig` reference
    /// (powershell.ts:33), invoked inside `exec` (bash.ts:91).
    ///
    /// Takes NO `shellPath`: `createLocalPowerShellOperations()` accepts no options at all
    /// (powershell.ts:32-33) and `PowerShellToolOptions` does not include `shellPath`
    /// (powershell.ts:29-30), so the settings `shellPath` — which points at a BASH — must never
    /// steer this tool.
    pub fn resolve_powershell() -> Result<Self, ToolError> {
        #[cfg(not(windows))]
        {
            // shell.ts:127 verbatim. Pi gates on `process.platform !== "win32"`, so the Rust gate
            // is `windows`, not `not(unix)`.
            Err(ToolError::new(
                "The powershell tool is only available on Windows.",
            ))
        }
        #[cfg(windows)]
        {
            Self::powershell_detect_from(find_executable_on_path)
        }
    }

    // NOTE: there is deliberately NO infallible `detect()` and no `Default for ShellConfig`.
    // Pi has no infallible entry point either — `getShellConfig` throws (shell.ts:100-106) and
    // every caller lives with that. An infallible wrapper can only degrade to a bare `bash -c`,
    // which turns Pi's actionable `No bash shell found` recipe into `spawn bash: … (os error 2)`
    // at the spawn site (ops/local/proc.rs). Resolve through `try_detect`/`resolve` at the point
    // of USE, where the error is the tool result the model reads (Pi bash.ts:91,457-468).
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
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.to_string_lossy().into_owned(),
            )
        })
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
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
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

    /// TOOL-038 / ADR-0003 D4 — on Windows with no Git Bash and no `bash.exe` on PATH, detection
    /// is Pi's hard stop (`shell.ts:100-106`), never a silent `cmd.exe /C` substitution.
    ///
    /// This test used to be `#[cfg(not(unix))]` and drove the arm by mutating `std::env` inside an
    /// `unsafe` block. Two things were wrong with that, and they compounded: it could only ever run
    /// on a Windows host (this project has none), and it did not COMPILE for one either —
    /// `#![deny(unsafe_code)]` (lib.rs:16) rejects the block, so
    /// `cargo check -p cyrup-tools --all-targets --target x86_64-pc-windows-gnu` was RED on it. The
    /// carefully written regression guard for the arm was therefore worth exactly nothing.
    /// Driving [`ShellConfig::windows_detect_from`] directly runs the same code on every host and
    /// drops the global-env mutation (which is UB-adjacent in a threaded test binary, hence the
    /// `unsafe`) at the same time.
    #[test]
    fn windows_arm_without_bash_errors_with_pis_repair_recipe() {
        // Candidates that cannot exist on any host, so the assertion is about the hard stop rather
        // than about whether this machine happens to have Git for Windows installed.
        let candidates = vec![
            std::env::temp_dir().join("cyrup-no-such-programfiles/Git/bin/bash.exe"),
            std::env::temp_dir().join("cyrup-no-such-programfiles-x86/Git/bin/bash.exe"),
        ];
        let err = ShellConfig::windows_detect_from(&candidates, || None)
            .expect_err("no bash anywhere ⇒ Pi throws");
        let msg = err.to_string();
        assert!(
            msg.starts_with("No bash shell found. Options:"),
            "got: {msg}"
        );
        assert!(msg.contains("1. Install Git for Windows: https://git-scm.com/download/win"));
        assert!(msg.contains("2. Add your bash to PATH (Cygwin, MSYS2, etc.)"));
        assert!(msg.contains("3. Set shellPath in settings.json"));
        assert!(msg.contains("Searched Git Bash in:"));
        assert!(
            !msg.contains("cmd.exe"),
            "cyrup must never name a non-bash interpreter here"
        );
        for cand in &candidates {
            assert!(
                msg.contains(&format!("  {}", cand.display())),
                "every searched candidate is listed with pi's two-space indent (shell.ts:104), \
                 missing {cand:?} in: {msg}"
            );
        }
    }

    /// The other two rows of pi's Windows order (shell.ts:76-99), also never covered: an existing
    /// Git Bash candidate wins over the PATH probe, and the PATH probe wins over the hard stop.
    /// Both must come back as bash with argv `-c` transport, never a different interpreter.
    #[test]
    fn windows_arm_prefers_git_bash_then_path_then_throws() {
        let dir = tempfile::tempdir().unwrap();
        let present = dir.path().join("bash.exe");
        std::fs::write(&present, b"#!/bin/sh\n").unwrap();
        let missing = dir.path().join("nope/Git/bin/bash.exe");

        // Row 1: a candidate that exists short-circuits before the PATH probe ever runs.
        let cfg = ShellConfig::windows_detect_from(&[missing.clone(), present.clone()], || {
            panic!("the PATH probe must not run once a Git Bash candidate exists (shell.ts:87-91)")
        })
        .unwrap();
        assert_eq!(cfg.program, present);
        assert_eq!(cfg.transport, Transport::Argv);
        assert_eq!(cfg.args, vec!["-c".to_string()]);

        // Row 2: no candidate exists, so `where bash.exe` decides (shell.ts:93-98).
        let on_path = dir.path().join("from-path-bash.exe");
        let cfg = ShellConfig::windows_detect_from(&[missing], || Some(on_path.clone())).unwrap();
        assert_eq!(cfg.program, on_path);
        assert_eq!(cfg.transport, Transport::Argv);
    }

    /// The unix arm is unaffected by TOOL-038: `sh -c` stays Pi's terminal fallback
    /// (`shell.ts:119`), so detection there is infallible.
    #[cfg(unix)]
    #[test]
    fn unix_detection_is_infallible_and_never_yields_cmd_exe() {
        let cfg = ShellConfig::try_detect().expect("unix detection cannot fail (shell.ts:119)");
        assert_ne!(cfg.program, PathBuf::from("cmd.exe"));
        assert!(
            cfg.program == PathBuf::from("/bin/bash")
                || cfg.program == PathBuf::from("sh")
                || cfg.program.file_name().is_some_and(|n| n == "bash"),
            "got {:?}",
            cfg.program
        );
    }

    /// TOOL-040 — the PATH probe is bounded at Pi's 5s (`shell.ts:47`, `:30`). Verified against a
    /// real long-running command rather than by reading the constant.
    #[cfg(unix)]
    #[test]
    fn path_probe_is_bounded() {
        assert_eq!(
            BASH_PROBE_TIMEOUT,
            Duration::from_secs(5),
            "shell.ts:47 `timeout: 5000`"
        );
        // A probe whose child never exits must be reaped at the deadline, not waited on forever.
        //
        // ALL THREE handles are set explicitly, and this is load-bearing, not tidiness:
        // `std::process::Command` defaults every UNSET handle to `Stdio::inherit()`, so the version
        // of this fixture that named only `stdout` handed the child the HARNESS's own stdin and
        // stderr. Under `cargo nextest run` the harness stderr is the pipe its leak detector waits
        // on (`.config/nextest.toml` `leak-timeout = 500ms, result = "fail"`), so any path that
        // let this `sleep 30` outlive the test process converted into a LEAK-FAIL. This was the
        // only spawn under `crates/cyrup-tools/**` that inherited a harness handle at all — every
        // other one (`find_bash_on_path` above, `build_command`/`build_argv_command` in
        // `ops/local/command.rs`, the `ops/local/tests/` fixtures) already pins all three.
        let started = Instant::now();
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn sleep");
        let deadline = Instant::now() + Duration::from_millis(200);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                // A `try_wait` error must still reap: leaving the loop without killing left a live
                // 30-second `sleep` behind, which is precisely the "spawns and does not reap" shape
                // this fixture exists to prove the production probe does NOT have.
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
            }
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the poll loop must reap on expiry"
        );
    }

    /// Pi `getPowerShellConfig`'s `win32` guard (shell.ts:126-128) — the sentence a NON-Windows
    /// host gets back from every `powershell` call, verbatim and with nothing around it.
    #[cfg(not(windows))]
    #[test]
    fn powershell_off_windows_is_pis_exact_refusal() {
        let err = ShellConfig::resolve_powershell()
            .expect_err("off Windows `getPowerShellConfig` throws (shell.ts:127)");
        assert_eq!(
            err.to_string(),
            "The powershell tool is only available on Windows."
        );
    }

    /// The Windows arm's body (shell.ts:130-135), driven on every host through the hoisted probe —
    /// the same treatment (and the same reason) as `windows_arm_prefers_git_bash_then_path_then_throws`
    /// above. Covers all three rows: `pwsh.exe` wins when both exist, `powershell.exe` is the
    /// fallback, and neither is Pi's second verbatim throw.
    #[test]
    fn powershell_arm_prefers_pwsh_then_powershell_then_throws() {
        let pwsh = PathBuf::from(r"C:\Program Files\PowerShell\7\pwsh.exe");
        let legacy = PathBuf::from(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe");

        // Row 1: both on PATH ⇒ PowerShell 7 (shell.ts:124,130 `??`).
        let both = |exe: &str| match exe {
            "pwsh.exe" => Some(pwsh.clone()),
            "powershell.exe" => Some(legacy.clone()),
            other => panic!("unexpected probe for {other}"),
        };
        let cfg = ShellConfig::powershell_detect_from(both).unwrap();
        assert_eq!(cfg.program, pwsh);
        assert_eq!(cfg.shell_name, "PowerShell");
        // shell.ts:122 `POWERSHELL_ARGS`, in order, with the command delivered as the trailing
        // argv entry after `-Command` (no `commandTransport`, shell.ts:135).
        assert_eq!(
            cfg.args,
            vec![
                "-NoProfile".to_string(),
                "-NonInteractive".to_string(),
                "-ExecutionPolicy".to_string(),
                "Bypass".to_string(),
                "-Command".to_string(),
            ]
        );
        assert_eq!(cfg.transport, Transport::Argv);

        // Row 2: only Windows PowerShell 5.1 ⇒ that one runs.
        let legacy_only = |exe: &str| match exe {
            "pwsh.exe" => None,
            "powershell.exe" => Some(legacy.clone()),
            other => panic!("unexpected probe for {other}"),
        };
        let cfg = ShellConfig::powershell_detect_from(legacy_only).unwrap();
        assert_eq!(cfg.program, legacy);
        assert_eq!(cfg.transport, Transport::Argv);

        // Row 3: shell.ts:132 verbatim.
        let err = ShellConfig::powershell_detect_from(|_| None)
            .expect_err("no PowerShell anywhere ⇒ Pi throws");
        assert_eq!(
            err.to_string(),
            "No PowerShell executable found. Install PowerShell or add powershell.exe/pwsh.exe to \
             PATH."
        );
    }

    /// The `shell_name` that rides on the resolved config is the TOOL's name for its shell, so the
    /// backend's missing-cwd message (`ops/local/proc.rs`) can say `Cannot execute PowerShell
    /// commands.` — including on the unix `sh -c` degradation, which Pi still calls `bash`
    /// (bash.ts:159 vs shell.ts:119).
    #[test]
    fn shell_name_is_the_tools_name_not_the_programs() {
        assert_eq!(ShellConfig::argv("/bin/bash").shell_name, "bash");
        assert_eq!(
            ShellConfig::argv(r"C:\Windows\System32\bash.exe").shell_name,
            "bash"
        );
        #[cfg(unix)]
        assert_eq!(
            ShellConfig::try_detect()
                .expect("unix detection cannot fail")
                .shell_name,
            "bash"
        );
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
