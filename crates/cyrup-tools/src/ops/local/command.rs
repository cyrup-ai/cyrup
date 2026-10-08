//! OS command construction for the two [`ProcOps`] methods.
//!
//! The two builders differ in exactly one load-bearing way: [`build_command`] (the `bash`-tool /
//! shell path) installs the unix `setsid` process-group setup, and [`build_argv_command`] (the
//! WASM `exec` capability's direct-argv path) deliberately does not. See [`super::proc`]'s module
//! doc for the consumer-level reason.
//!
//! [`ProcOps`]: crate::ops::ProcOps

use crate::ops::win::windows_hide;
use crate::ops::{ArgvSpec, ExecSpec, Transport};

/// Build the OS command for an [`ExecSpec`], installing the unix process-group setup.
#[allow(unsafe_code)]
pub(super) fn build_command(spec: &ExecSpec) -> std::process::Command {
    let mut std_cmd = std::process::Command::new(&spec.shell.program);
    std_cmd.args(&spec.shell.args);
    if spec.shell.transport == Transport::Argv {
        std_cmd.arg(&spec.command);
    }
    std_cmd.current_dir(&spec.cwd);
    // Removals FIRST, then the overrides — Pi deletes the session keys and only then repopulates
    // them (bash.ts:165-181), so a key in both lists ends up set, not unset.
    for k in &spec.env_remove {
        std_cmd.env_remove(k);
    }
    for (k, v) in &spec.env {
        std_cmd.env(k, v);
    }
    std_cmd.stdout(std::process::Stdio::piped());
    std_cmd.stderr(std::process::Stdio::piped());
    if spec.shell.transport == Transport::Stdin {
        std_cmd.stdin(std::process::Stdio::piped());
    } else {
        std_cmd.stdin(std::process::Stdio::null());
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: `setsid` only detaches the child into its own session/process group before exec;
        // it touches no parent memory and is async-signal-safe. This makes the child the group
        // leader (pgid == pid) so the whole tree can be killed via `killpg` (R-03-027).
        unsafe {
            std_cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    // The Windows counterpart of the `setsid` block above, and the exact partner of the
    // `detached: process.platform !== "win32"` on the same spawn: Pi passes `windowsHide: true`
    // (bash.ts:104) for BOTH shell tools, so `bash -c` / `bash -s` never flashes a console.
    // `creation_flags` is safe and stable, so this adds no `unsafe` beyond the block above.
    windows_hide(&mut std_cmd);
    std_cmd
}

/// Detach a command the caller spawns and then forgets into a session of its own (`setsid(2)`):
/// Node's `spawn(…, { detached: true })` on POSIX, for a caller outside this crate's two
/// [`ProcOps`] builders. Its one consumer is `cyrup-tui`'s browser launcher (pi
/// `open-browser.ts:21`), whose crate is `#![forbid(unsafe_code)]` and so cannot install the
/// `pre_exec` hook itself.
///
/// A session, not merely a process group: the child has NO controlling terminal, so whatever it
/// starts (`xdg-open`'s console-browser fallback on a host with no `DISPLAY`) cannot open
/// `/dev/tty` at all (`ENXIO`). A process group alone relies on job-control signals stopping a
/// background reader or `tcsetattr` caller, and those are no defence when the parent inherited
/// `SIGTTIN`/`SIGTTOU` as ignored — `tmux new-session cyrup` does exactly that (tmux 3.4 resets
/// neither for the pane command), and then a background `tcsetattr` simply succeeds and re-modes
/// the TUI's terminal. A no-op off unix.
///
/// [`ProcOps`]: crate::ops::ProcOps
#[allow(unsafe_code)]
pub fn detach_into_new_session(cmd: &mut std::process::Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: as in [`build_command`] — `setsid` runs in the forked child before exec, touches
        // no parent memory and is async-signal-safe. It can fail only with `EPERM` when the child
        // already leads a process group, which a freshly forked child never does.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    #[cfg(not(unix))]
    {
        let _ = cmd;
    }
}

/// Build the OS command for an [`ArgvSpec`] — a DIRECT argv (shell:false) exec (Pi `execCommand`
/// spawn with `shell:false`, exec.ts:41-45): the program IS `spec.program`, its args are the literal
/// `spec.args` (no shell, no word-splitting). Unlike [`build_command`] (the `bash`-tool/shell path,
/// whose real consumer `bash.ts:97-99` passes `detached: true`), this deliberately does NOT
/// `setsid` the child — Pi's real `execCommand` (`exec.ts:41-45`) never sets `detached` either, so
/// the spawned process stays in the caller's own process group and [`LocalProc::exec_argv`]'s
/// escalation targets it by single pid only ([`terminate_pid`]/[`kill_pid`]), never `killpg`.
///
/// [`LocalProc::exec_argv`]: crate::ops::ProcOps::exec_argv
/// [`kill_pid`]: crate::ops::local::signal::kill_pid
/// [`terminate_pid`]: crate::ops::local::signal::terminate_pid
pub(super) fn build_argv_command(spec: &ArgvSpec) -> std::process::Command {
    let mut std_cmd = std::process::Command::new(&spec.program);
    std_cmd.args(&spec.args);
    // An empty `cwd` is skipped rather than passed to `current_dir` — matching Node's real
    // `child_process.spawn`, which treats a falsy `cwd` as "no override" and inherits the parent's
    // own ambient cwd (verified live: Node `spawn("pwd",[],{cwd:""})` exits 0, printing the ambient
    // cwd), the exact real-consumer behavior `execCommand` (`exec.ts:41-45`) relies on. Unlike Node,
    // `std::process::Command::current_dir("")` hard-fails the spawn (verified live: `Os { code: 2,
    // kind: NotFound, .. }`) — this callers-owned defensive check (this crate has no upstream
    // knowledge of WHY `spec.cwd` might be empty; `cyrup-session-svc::host_services::exec` already
    // folds a guest-supplied empty `cwd` back to the session cwd before building an `ArgvSpec`, so
    // this is defense in depth for any other/future caller of `exec_argv`) keeps that same graceful
    // degrade rather than erroring on a `PathBuf::new()`.
    if !spec.cwd.as_os_str().is_empty() {
        std_cmd.current_dir(&spec.cwd);
    }
    for (k, v) in &spec.env {
        std_cmd.env(k, v);
    }
    // Pi uses stdio `["ignore","pipe","pipe"]` (exec.ts:44): stdin closed, stdout+stderr piped.
    std_cmd.stdin(std::process::Stdio::null());
    std_cmd.stdout(std::process::Stdio::piped());
    std_cmd.stderr(std::process::Stdio::piped());
    // Deliberately NO `setsid`/process-group setup here — Pi's real `execCommand` spawn
    // (`exec.ts:41-45`) never sets `detached`, so the child stays in the caller's own process
    // group and must be signaled by single pid only, never `killpg` (see the doc comment above and
    // `super::proc`'s module doc).
    //
    // Deliberately NO `windows_hide` either, for the same 1:1-with-its-own-consumer reason: that
    // same `exec.ts:41-45` spawn passes no `windowsHide`, and Node's default is `false`, so this
    // child SHOWS a console window upstream. Adding the flag here would be a behavior Pi does not
    // have. Contrast `build_command` above, whose consumer (`bash.ts:104`) does pass it.
    std_cmd
}
