//! Open a URL in the platform browser / default handler — a port of pi
//! `coding-agent/src/utils/open-browser.ts:10-24` (`openBrowser`), read at **v0.84.2** and
//! byte-identical at the ported tag **v0.83.0**.
//!
//! # The mechanism, not the vibe
//!
//! Upstream is four lines of decision and one of execution, and every clause is load-bearing:
//!
//! ```text
//! const [cmd, args]: [string, string[]] =
//!     process.platform === "darwin"
//!         ? ["open", [target]]
//!         : process.platform === "win32"
//!             ? ["rundll32", ["url.dll,FileProtocolHandler", target]]
//!             : ["xdg-open", [target]];
//! spawn(cmd, args, { stdio: "ignore", detached: true }).on("error", () => {}).unref();
//! ```
//!
//! * **Never through a shell.** pi's doc comment says so and says why: "On Windows, do not use
//!   `cmd /c start`: cmd.exe re-parses metacharacters (&, |, ^, ...) before `start` runs, which
//!   would make attacker-controlled URLs injectable" (`open-browser.ts:5-8`). An OAuth authorize
//!   URL is provider-supplied, so this is the security property of the function, not a style note.
//!   [`open_browser`] therefore uses [`std::process::Command`] with an argv vector and no shell.
//! * **Detached and unreferenced.** The launcher must outlive the call and must not keep the
//!   process alive. `detached: true` + `.unref()` is Node's spelling; the Rust one is a spawn whose
//!   [`Child`](std::process::Child) is handed to [`reap`] rather than dropped — see below.
//! * **Off the TUI's terminal.** On POSIX, Node's `detached: true` is `setsid(2)`: the launcher
//!   starts a session of its own and has no controlling terminal, so nothing it starts can read
//!   keystrokes or re-mode the tty the TUI owns. That matters most exactly where the copy-code
//!   login is for — an SSH session or a container with no `DISPLAY`, where `xdg-open` falls back
//!   to a console browser (`www-browser`/`lynx`/`w3m`) that would otherwise grab `/dev/tty`
//!   underneath the raw-mode TUI. See [`detach`] for the `forbid(unsafe_code)` spelling.
//! * **Best-effort.** `.on("error", () => {})` swallows a missing `xdg-open`; the caller has
//!   already printed the URL, so a launcher failure must never surface. [`open_browser`] returns
//!   `()` and discards every error for the same reason.
//!
//! # `[CYRUP-DELTA]` — a reaper thread where Node has `unref()`
//!
//! Node's `detached: true` reparents the child, so libuv never has to wait on it. On Unix a
//! [`std::process::Child`] that is merely dropped is **not** reaped and stays a zombie in the
//! process table until cyrup exits — a `/login` per session is not a leak that matters, but the
//! same function is reachable from an extension's `openUrl` (pi `interactive-mode.ts:353` binds
//! `openBrowser` straight onto the extension context), which is unbounded. So the spawn is handed
//! to a detached thread that waits once and exits. That is a Rust-lifecycle requirement Node does
//! not have; the observable behaviour — a browser opens, failures are silent, the caller is never
//! blocked — is upstream's.

use std::process::{Command, Stdio};

/// The launcher argv for `os` (an [`std::env::consts::OS`] value) and `target`.
///
/// `os` is a parameter rather than a `cfg!` for the reason [`crate::clipboard::clipboard_write_plan`]
/// already states: a target-gated arm nobody can execute is how a platform branch rots. The Windows
/// and Linux argvs are asserted from a macOS host.
///
/// pi's ternary is `darwin` → `win32` → everything else, so every non-Windows unix (Linux, the
/// BSDs, Solaris) takes the `xdg-open` arm exactly as upstream does.
pub(crate) fn browser_command(os: &str, target: &str) -> (&'static str, Vec<String>) {
    match os {
        // `["open", [target]]` (`open-browser.ts:13`).
        "macos" => ("open", vec![target.to_string()]),
        // `["rundll32", ["url.dll,FileProtocolHandler", target]]` (`open-browser.ts:15`). The
        // handler spec and the target are two SEPARATE argv entries upstream; joining them would
        // change what rundll32 parses.
        "windows" => (
            "rundll32",
            vec![
                "url.dll,FileProtocolHandler".to_string(),
                target.to_string(),
            ],
        ),
        // `["xdg-open", [target]]` (`open-browser.ts:16`).
        _ => ("xdg-open", vec![target.to_string()]),
    }
}

/// Open `target` in the platform browser. Best-effort and non-blocking: a missing launcher, a
/// headless session or a non-zero exit are all silently ignored (pi `.on("error", () => {})`).
pub fn open_browser(target: &str) {
    let (bin, args) = browser_command(std::env::consts::OS, target);
    launch(bin, &args);
}

/// The configured, not-yet-spawned launcher: argv only (no shell), every stdio stream discarded,
/// detached from the TUI's terminal. Split from [`launch`] so a test can spawn the exact
/// configuration with a probe binary in place of the platform launcher.
fn launcher(bin: &str, args: &[String]) -> Command {
    let mut command = Command::new(bin);
    command
        .args(args)
        // `stdio: "ignore"` (`open-browser.ts:21`) — a launcher must never write into the raw-mode
        // TUI's terminal.
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    detach(&mut command);
    command
}

/// Spawn [`launcher`] and hand the child to [`reap`]. A spawn error — `xdg-open` not installed, as
/// in most containers — is dropped on the floor, which is upstream's `.on("error", () => {})`.
fn launch(bin: &str, args: &[String]) {
    if let Ok(child) = launcher(bin, args).spawn() {
        reap(child);
    }
}

/// `detached: true` (`open-browser.ts:21`), which Node implements on POSIX with `setsid(2)`. This
/// crate is `#![forbid(unsafe_code)]`, which rules out the `pre_exec` hook `setsid` needs here, so
/// the call is [`cyrup_tools::detach_into_new_session`] (that crate isolates its unix process
/// `unsafe`). The launcher then has no controlling terminal: a console browser it falls back to
/// cannot even open `/dev/tty`, let alone steal the user's keystrokes or switch the terminal out of
/// the TUI's raw mode, and terminal-generated signals never reach it.
///
/// A process group alone (`Command::process_group(0)`, the safe call) was the first spelling and is
/// not enough: it leans on `SIGTTIN`/`SIGTTOU` stopping a background group that touches the tty,
/// and a TUI started straight from `tmux new-session cyrup` inherits both signals as *ignored*
/// (tmux 3.4 resets neither for a pane command), so the launcher's `tcsetattr` simply succeeded.
#[cfg(unix)]
fn detach(command: &mut Command) {
    cyrup_tools::detach_into_new_session(command);
}

/// Windows: `rundll32` has no console to share, so there is nothing to detach from.
#[cfg(not(unix))]
fn detach(_command: &mut Command) {}

/// The `[CYRUP-DELTA]` half of `detached: true` + `.unref()`: wait for the launcher on a detached
/// thread so it is reaped, without blocking the caller and without keeping a handle alive.
fn reap(mut child: std::process::Child) {
    let _ = std::thread::Builder::new()
        .name("cyrup-open-browser".to_string())
        .spawn(move || {
            let _ = child.wait();
        });
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// **DRIFT-042.** Pi's three platform argvs, all three asserted from one host.
    ///
    /// **Red before the fix:** `crates/cyrup-tui/src/open_browser.rs` did not exist, and
    /// `grep -rnE 'xdg-open|rundll32|open_browser|FileProtocolHandler' crates --include='*.rs'`
    /// returned 0 hits across the whole workspace — the test could not compile, let alone pass.
    #[test]
    fn the_three_platform_argvs_match_open_browser_ts() {
        let url = "https://example.com/oauth/authorize?code=1";
        assert_eq!(
            browser_command("macos", url),
            ("open", vec![url.to_string()])
        );
        assert_eq!(
            browser_command("windows", url),
            (
                "rundll32",
                vec!["url.dll,FileProtocolHandler".to_string(), url.to_string()],
            )
        );
        assert_eq!(
            browser_command("linux", url),
            ("xdg-open", vec![url.to_string()])
        );
        // pi's ternary has no fourth arm: every remaining platform falls to `xdg-open`.
        for other in ["freebsd", "openbsd", "netbsd", "solaris", "android"] {
            assert_eq!(
                browser_command(other, url).0,
                "xdg-open",
                "{other} must take pi's else-arm"
            );
        }
    }

    /// The target is one argv entry, never spliced into a command string — pi's stated security
    /// property (`open-browser.ts:5-8`). A URL carrying shell metacharacters must survive intact
    /// and must not gain any quoting, because nothing re-parses it.
    #[test]
    fn a_url_with_shell_metacharacters_is_passed_as_one_unquoted_argv_entry() {
        let hostile = "https://example.com/cb?a=1&b=2|c^d;e`f`";
        for os in ["macos", "windows", "linux"] {
            let (_, args) = browser_command(os, hostile);
            assert!(
                args.contains(&hostile.to_string()),
                "{os}: the target must appear verbatim as its own argv entry"
            );
            assert!(
                args.iter().all(|a| !a.contains(' ') || a == hostile),
                "{os}: no argv entry may be a joined command string"
            );
        }
        // rundll32 keeps the handler spec separate from the target (two entries, not one).
        assert_eq!(browser_command("windows", hostile).1.len(), 2);
    }

    /// The headless case the copy-code login exists for: no launcher on `PATH` (a container, a
    /// minimal SSH host). The spawn error is swallowed, nothing is printed, and the call returns
    /// at once — the auth URL the dialog already staged stays the way in.
    #[test]
    fn a_missing_launcher_is_silent_and_does_not_block() {
        let started = std::time::Instant::now();
        launch(
            "cyrup-test-no-such-browser-launcher",
            &["https://example.test/oauth/authorize".to_string()],
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "a failed spawn must not block the caller"
        );
    }

    /// `detached: true` — the launcher leads a session of its own with no controlling terminal, so
    /// a console browser `xdg-open` falls back to without a `DISPLAY` cannot open `/dev/tty`, read
    /// the user's keystrokes or re-mode the terminal — even when the TUI inherited
    /// `SIGTTIN`/`SIGTTOU` as ignored (`tmux new-session cyrup`), where a mere process group is no
    /// defence.
    ///
    /// **Red before the fix:** `detach` was `Command::process_group(0)`, so the probe's session id
    /// was the test process's own, not its pid, and it kept the controlling terminal.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_launcher_leads_its_own_session_without_a_controlling_terminal() {
        /// `/proc/<pid>/stat` fields after the `(comm)`: state, ppid, pgrp, session, tty_nr.
        fn stat_fields(stat: &str) -> Vec<i64> {
            stat.rsplit(')')
                .next()
                .unwrap()
                .split_whitespace()
                .skip(1)
                .take(4)
                .map(|f| f.parse().unwrap())
                .collect()
        }
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("stat");
        // `$0` is the output path; the probe copies its own stat line.
        let mut child = launcher(
            "sh",
            &[
                "-c".to_string(),
                r#"cat "/proc/$$/stat" > "$0""#.to_string(),
                out.display().to_string(),
            ],
        )
        .spawn()
        .unwrap();
        let pid = i64::from(child.id());
        assert!(child.wait().unwrap().success());
        let probe = stat_fields(&std::fs::read_to_string(&out).unwrap());
        let own = stat_fields(&std::fs::read_to_string("/proc/self/stat").unwrap());
        let [_, pgrp, session, tty_nr] = probe[..] else {
            panic!("short stat line: {probe:?}")
        };
        let [_, _, own_session, _] = own[..] else {
            panic!("short stat line: {own:?}")
        };
        assert_eq!(pgrp, pid, "the launcher must lead its own process group");
        assert_eq!(
            session, pid,
            "the launcher must lead its own session (setsid)"
        );
        assert_ne!(session, own_session, "never the TUI's own session");
        assert_eq!(tty_nr, 0, "the launcher must have no controlling terminal");
    }
}
