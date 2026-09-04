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
    let spawned = Command::new(bin)
        .args(&args)
        // `stdio: "ignore"` (`open-browser.ts:22`) — a launcher must never write into the raw-mode
        // TUI's terminal.
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if let Ok(child) = spawned {
        reap(child);
    }
}

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
}
