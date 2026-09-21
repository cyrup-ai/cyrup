//! Rendering an executable + argv as ONE shell command line (pi `src/inspectors/shell-command.ts`,
//! 16 lines @v0.68.0).
//!
//! This is not a cosmetic formatter. The string it produces is what a terminal HOST is asked to
//! type into a live shell — `herdr pane run <paneId> <displayCommand>` types it and presses Enter
//! (`inspectors/herdr/actions.ts:111`; herdr's own `pane run` is `pane.send_input` with
//! `keys: ["Enter"]`, `tmp/herdr/src/cli/pane.rs:1046-1052` @`d59d060`), and ghostty hands it to
//! AppleScript as `command of surfaceConfiguration` (`inspectors/ghostty/actions.ts:15,61`). A
//! mis-quoted token is therefore not a display bug; it is a command the user's shell runs wrong.
//!
//! Both of upstream's branches are ported, and [`Platform`] is a parameter rather than a
//! `cfg!`, so the PowerShell branch is exercised by this module's own tests on a Linux CI box.

use super::types::Platform;

/// The [`Platform`] branch this build's host takes when a caller does not name one.
///
/// A free function rather than an inherent method on [`Platform`], because [`Platform`] lives in
/// the frozen contract (`inspectors/types.rs`) and a second module adding an inherent `impl` for
/// it would collide with any other implementation module that did the same.
///
/// `cfg!` rather than a runtime probe, matching pi's `process.platform` default
/// (`shell-command.ts:10`): the quoting rules belong to the shell the launch string will be typed
/// into, which is the host's shell.
#[must_use]
pub const fn host_platform() -> Platform {
    if cfg!(windows) {
        Platform::Win32
    } else {
        Platform::Unix
    }
}

/// pi `shellQuote` (`shell-command.ts:1-4`).
///
/// PowerShell: wrap in double quotes and backslash-escape embedded double quotes. POSIX: wrap in
/// single quotes and close/escape/reopen for each embedded single quote (`'\''`).
fn shell_quote(value: &str, platform: Platform) -> String {
    match platform {
        Platform::Win32 => format!("\"{}\"", value.replace('"', "\\\"")),
        Platform::Unix => format!("'{}'", value.replace('\'', "'\\''")),
    }
}

/// pi `isBareExecutable` (`shell-command.ts:6-8`) — `/^[\w./@:-]+$/`.
///
/// JavaScript's `\w` without the `u` flag is exactly `[A-Za-z0-9_]`, so the accepted set is ASCII
/// alphanumerics plus `_ . / @ : -`. The `+` makes the empty string NOT bare, which matters: an
/// empty executable must take the `sh -c` branch rather than render as a bare nothing.
fn is_bare_executable(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '@' | ':' | '-'))
}

/// pi `formatShellCommand` (`shell-command.ts:10-16`).
///
/// * `Win32` → `& "<exe>" "<arg>" …` — PowerShell's call operator, which is required because a
///   quoted first token is otherwise a string literal PowerShell merely echoes.
/// * `Unix` → `<exe> '<arg>' …`, except that an executable needing quotes is invoked through
///   `sh -c 'exec "$0" "$@"' '<exe>'`. That indirection is upstream's own comment
///   (`shell-command.ts:13`): *"Nushell treats a leading quoted token as a string, so use a bare
///   invoker for paths that need quoting."* Nushell is a plausible shell inside a herdr pane, and
///   there a quoted leading token prints the path instead of running it.
#[must_use]
pub fn format_shell_command(exe: &str, args: &[String], platform: Platform) -> String {
    let quoted_args = args.iter().map(|arg| shell_quote(arg, platform));
    match platform {
        Platform::Win32 => {
            let mut parts = vec![shell_quote(exe, platform)];
            parts.extend(quoted_args);
            format!("& {}", parts.join(" "))
        }
        Platform::Unix => {
            let invocation = if is_bare_executable(exe) {
                exe.to_string()
            } else {
                format!("sh -c 'exec \"$0\" \"$@\"' {}", shell_quote(exe, platform))
            };
            let mut parts = vec![invocation];
            parts.extend(quoted_args);
            parts.join(" ")
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    /// T-SHELL-1 — the POSIX branch, bare executable. GUT `is_bare_executable` to always return
    /// `false` and this goes RED, because the common case would grow an `sh -c` wrapper.
    #[test]
    fn a_bare_unix_executable_is_invoked_directly_with_quoted_args() {
        let rendered = format_shell_command(
            "/usr/local/bin/cyrup",
            &args(&["__subagent-inspector", "--async-dir", "/runs/a b"]),
            Platform::Unix,
        );
        assert_eq!(
            rendered,
            "/usr/local/bin/cyrup '__subagent-inspector' '--async-dir' '/runs/a b'"
        );
    }

    /// T-SHELL-2 — the nushell workaround. GUT `is_bare_executable` to always return `true` and
    /// this goes RED; nothing else in the suite sees the difference, and the failure mode in the
    /// field is a pane that prints the path instead of running the inspector.
    #[test]
    fn a_unix_executable_needing_quotes_is_invoked_through_sh_c() {
        let rendered = format_shell_command(
            "/opt/my apps/cyrup",
            &args(&["--run-id", "abc"]),
            Platform::Unix,
        );
        assert_eq!(
            rendered,
            "sh -c 'exec \"$0\" \"$@\"' '/opt/my apps/cyrup' '--run-id' 'abc'"
        );
    }

    /// POSIX single-quote escaping is the close/escape/reopen dance, not a backslash.
    #[test]
    fn a_unix_argument_with_a_single_quote_is_closed_escaped_and_reopened() {
        let rendered = format_shell_command("/bin/cyrup", &args(&["it's"]), Platform::Unix);
        assert_eq!(rendered, r#"/bin/cyrup 'it'\''s'"#);
    }

    /// T-SHELL-3 — the PowerShell branch, reachable from Linux CI because [`Platform`] is a
    /// parameter. GUT the `&` call operator away and this goes RED; on a real Windows host that
    /// mutation turns the whole launch into a string PowerShell echoes.
    #[test]
    fn the_win32_branch_uses_the_call_operator_and_double_quotes() {
        let rendered = format_shell_command(
            r"C:\Program Files\cyrup.exe",
            &args(&["--async-dir", r"C:\runs\a"]),
            Platform::Win32,
        );
        assert_eq!(
            rendered,
            r#"& "C:\Program Files\cyrup.exe" "--async-dir" "C:\runs\a""#
        );
    }

    /// PowerShell embeds a literal double quote as `\"` in upstream's quoter — ported verbatim,
    /// including the fact that it does NOT escape backslashes (see the `session_roots_codec`
    /// module doc for why the one payload that would break under that is base64 instead).
    #[test]
    fn a_win32_argument_with_a_double_quote_is_backslash_escaped() {
        let rendered = format_shell_command("cyrup.exe", &args(&[r#"say "hi""#]), Platform::Win32);
        assert_eq!(rendered, r#"& "cyrup.exe" "say \"hi\"""#);
    }

    /// Every character of upstream's bare set, and one that is not in it.
    #[test]
    fn the_bare_executable_set_is_upstreams_exactly() {
        assert!(is_bare_executable("aZ09_./@:-"));
        assert!(!is_bare_executable(""));
        assert!(!is_bare_executable("has space"));
        assert!(!is_bare_executable(r"C:\cyrup.exe"));
        assert!(!is_bare_executable("caf\u{e9}"));

        // BOTH directions, over the whole of ASCII. The accepted-side rows above cannot see a
        // WIDENED set: adding `%` to the `matches!` leaves every one of them green while
        // production starts invoking `weird%path` bare instead of through `sh -c`. `/^[\w./@:-]+$/`
        // with no `u` flag is `[A-Za-z0-9_]` plus `. / @ : -`, so this is upstream's regex as a
        // predicate rather than as five examples.
        for byte in 0u8..=127 {
            let ch = char::from(byte);
            let expected =
                ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '/' | '@' | ':' | '-');
            assert_eq!(
                is_bare_executable(&ch.to_string()),
                expected,
                "U+{byte:04X} ({ch:?}) is on the wrong side of upstream's /^[\\w./@:-]+$/"
            );
        }
    }

    /// An empty argv still renders a runnable command (the exe alone), on both branches.
    #[test]
    fn an_empty_argv_renders_the_executable_alone() {
        assert_eq!(format_shell_command("cyrup", &[], Platform::Unix), "cyrup");
        assert_eq!(
            format_shell_command("cyrup", &[], Platform::Win32),
            "& \"cyrup\""
        );
    }

    /// [`host_platform`] is the default the production caller uses; it must be one of the two
    /// branches and must agree with the build target.
    #[test]
    fn the_host_platform_matches_the_build_target() {
        let expected = if cfg!(windows) {
            Platform::Win32
        } else {
            Platform::Unix
        };
        assert_eq!(host_platform(), expected);
    }
}
