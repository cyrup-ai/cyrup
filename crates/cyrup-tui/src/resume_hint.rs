//! The resume-command hint printed on the way out of interactive mode — the port of Pi's
//! `formatResumeCommand` (`coding-agent/src/modes/interactive/interactive-mode.ts:231-244`) and of
//! the `shutdown()` write that uses it (`:3594-3597`).
//!
//! # What it is for
//!
//! Pi's last act before `process.exit(0)` is one line on stdout:
//!
//! ```text
//! To resume this session: pi --session 0199f0c4-...
//! ```
//!
//! …or, when the session does not live in the cwd-encoded default directory, the full invocation
//! including the directory:
//!
//! ```text
//! To resume this session: cyrup --session-dir /srv/shared/sessions --session 0199f0c4-...
//! ```
//!
//! That second form is the one that matters. A session written under an explicit `--session-dir` is
//! not reachable from the picker a bare relaunch offers — `SessionManager.list` only walks the
//! session's *own* directory — so without this line the user has to reconstruct both the directory
//! and a UUID they were never shown. Printing it is the only surfaced route back.
//!
//! # The three gates, and why each one is there
//!
//! [`format_resume_command`] returns `None` unless all three hold, exactly as Pi's does:
//!
//! 1. **stdout is a tty** (`:232`). Redirected output is somebody's data; a decorative hint would
//!    corrupt it.
//! 2. **the session is persisted** (`:233`) — `--no-session` has nothing to resume.
//! 3. **the session file exists on disk** (`:235-236`). A session whose file was never flushed (an
//!    immediate quit) or was deleted mid-run cannot be resumed, and offering a command that fails is
//!    worse than offering none.
//!
//! # Quoting
//!
//! [`quote_if_needed`] is Pi's (`:224-229`): a value made only of `A-Za-z0-9_-./~:@` is emitted
//! bare, anything else is single-quoted with embedded quotes escaped as `'\''`. The command is meant
//! to be pasted into a shell, so a session directory with a space in it has to survive the trip.

use std::path::Path;

/// The binary name the hint tells the user to run. Pi's `APP_NAME` (`"pi"`); cyrup's rebrand makes it
/// `"cyrup"`, matching `crates/cyrup/src/startup.rs:22`.
pub const APP_NAME: &str = "cyrup";

/// Everything [`format_resume_command`] needs about the live session, so the formatter itself stays a
/// pure function of already-resolved values (Pi reads the same five things off `SessionManager`).
#[derive(Clone, Copy, Debug)]
pub struct ResumeTarget<'a> {
    /// The session's id — Pi `getSessionId()`, the `--session` argument.
    pub session_id: &'a str,
    /// The session's on-disk file, or `None` for an unpersisted (`--no-session`) run. Covers BOTH of
    /// Pi's checks: `isPersisted()` and `getSessionFile()` (`:233-235`).
    pub session_file: Option<&'a Path>,
    /// The directory this session's files live in — Pi `getSessionDir()`.
    pub session_dir: &'a Path,
    /// The directory this session WOULD live in with no `--session-dir` override: the cwd-encoded
    /// default. Pi computes it inline as `getDefaultSessionDirPath(this.cwd)` inside
    /// `usesDefaultSessionDir()` (`session-manager.ts:1003-1005`); it is passed in here because
    /// only the caller knows the sessions root and cwd.
    pub default_session_dir: &'a Path,
}

/// Pi `formatResumeCommand` (`interactive-mode.ts:231-244`): the exact command line that returns
/// the user to this session, or `None` when there is nothing resumable to point at.
///
/// `stdout_is_tty` is Pi's `process.stdout.isTTY` lifted to a parameter — the caller owns the
/// syscall, and passing it makes the gate testable.
pub fn format_resume_command(target: &ResumeTarget<'_>, stdout_is_tty: bool) -> Option<String> {
    if !stdout_is_tty {
        return None;
    }
    // `isPersisted()` + `getSessionFile()` + `existsSync(sessionFile)` (`:233-236`).
    let file = target.session_file?;
    if !file.exists() {
        return None;
    }
    let mut command = String::from(APP_NAME);
    // `if (!sessionManager.usesDefaultSessionDir()) args.push("--session-dir", …)` (`:239-241`).
    if target.session_dir != target.default_session_dir {
        command.push_str(" --session-dir ");
        command.push_str(&quote_if_needed(&target.session_dir.to_string_lossy()));
    }
    // Pi pushes the id RAW (`args.push("--session", sessionManager.getSessionId())`, `:242`) — only
    // the directory goes through `quoteIfNeeded`. Kept as-is: a session id is a UUID, so quoting
    // would be a no-op on every real value, and diverging here would print a command Pi would not.
    command.push_str(" --session ");
    command.push_str(target.session_id);
    Some(command)
}

/// The line Pi writes on stdout (`interactive-mode.ts:3596`):
/// `${chalk.dim("To resume this session:")} ${resumeCommand}\n`.
///
/// `chalk.dim` is SGR 2, closed by SGR 22 — the same pair chalk emits. Only the label is dimmed; the
/// command itself stays at normal intensity so it reads as copyable text, which is what chalk's
/// template does.
pub fn resume_hint_line(command: &str) -> String {
    format!("\x1b[2mTo resume this session:\x1b[22m {command}\n")
}

/// Pi `quoteIfNeeded` (`interactive-mode.ts:224-229`): leave a shell-safe, non-empty value alone;
/// otherwise single-quote it, ending and reopening the quote around each embedded `'` (`'\''`).
///
/// Pi's safe set is the regex character class `[a-zA-Z0-9_\-./~:@]`. An empty string fails its
/// `value.length > 0` guard and so comes back as `''` — which is correct, and is what makes an empty
/// argument survive as an argument at all rather than vanishing.
pub fn quote_if_needed(value: &str) -> String {
    let safe = |c: char| {
        c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '~' | ':' | '@')
    };
    if !value.is_empty() && value.chars().all(safe) {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

    use super::*;

    /// A session in the cwd-encoded default directory needs only `--session`, exactly as Pi's
    /// `usesDefaultSessionDir()` branch decides.
    #[test]
    fn the_default_directory_yields_a_bare_session_flag() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("s.jsonl");
        std::fs::write(&file, b"{}").unwrap();
        let target = ResumeTarget {
            session_id: "0199f0c4-dead-beef",
            session_file: Some(&file),
            session_dir: dir.path(),
            default_session_dir: dir.path(),
        };
        assert_eq!(
            format_resume_command(&target, true).as_deref(),
            Some("cyrup --session 0199f0c4-dead-beef")
        );
    }

    /// The case the hint exists for: under an explicit `--session-dir` the printed command must
    /// carry the directory too, or the session is unreachable.
    #[test]
    fn a_custom_directory_is_carried_into_the_command() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("s.jsonl");
        std::fs::write(&file, b"{}").unwrap();
        let target = ResumeTarget {
            session_id: "abc",
            session_file: Some(&file),
            session_dir: dir.path(),
            default_session_dir: Path::new("/home/u/.cyrup/sessions/--home-u-proj--"),
        };
        let command = format_resume_command(&target, true).expect("a resumable session");
        assert!(
            command.contains("--session-dir"),
            "a session outside the default dir is not reachable from a bare relaunch, so the \
             directory must be printed: {command}"
        );
        assert_eq!(command, format!("cyrup --session-dir {} --session abc", dir.path().display()));
    }

    /// A directory a shell would mangle is quoted Pi's way.
    #[test]
    fn an_unsafe_directory_is_single_quoted() {
        let dir = tempfile::tempdir().unwrap();
        let spaced = dir.path().join("my sessions");
        std::fs::create_dir_all(&spaced).unwrap();
        let file = spaced.join("s.jsonl");
        std::fs::write(&file, b"{}").unwrap();
        let target = ResumeTarget {
            session_id: "abc",
            session_file: Some(&file),
            session_dir: &spaced,
            default_session_dir: Path::new("/elsewhere"),
        };
        let command = format_resume_command(&target, true).unwrap();
        assert!(command.contains(&format!("'{}'", spaced.display())), "{command}");
    }

    /// `quoteIfNeeded`'s full contract, including the `'\''` escape and the empty-string case.
    #[test]
    fn quoting_matches_pi() {
        assert_eq!(quote_if_needed("/home/u/.cyrup/sessions"), "/home/u/.cyrup/sessions");
        assert_eq!(quote_if_needed("a-b_c.d~e:f@g"), "a-b_c.d~e:f@g");
        assert_eq!(quote_if_needed("with space"), "'with space'");
        assert_eq!(quote_if_needed("it's"), r"'it'\''s'");
        assert_eq!(quote_if_needed(""), "''");
    }

    /// Gate 1: redirected stdout is data, not a place for a decoration.
    #[test]
    fn a_non_tty_stdout_prints_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("s.jsonl");
        std::fs::write(&file, b"{}").unwrap();
        let target = ResumeTarget {
            session_id: "abc",
            session_file: Some(&file),
            session_dir: dir.path(),
            default_session_dir: dir.path(),
        };
        assert_eq!(format_resume_command(&target, false), None);
    }

    /// Gates 2 and 3: an unpersisted run, and a persisted one whose file is not on disk, are both
    /// unresumable — offering a command that would fail is worse than offering none.
    #[test]
    fn an_unpersisted_or_missing_session_file_prints_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let unpersisted = ResumeTarget {
            session_id: "abc",
            session_file: None,
            session_dir: dir.path(),
            default_session_dir: dir.path(),
        };
        assert_eq!(format_resume_command(&unpersisted, true), None);

        let vanished = dir.path().join("never-written.jsonl");
        let missing = ResumeTarget {
            session_id: "abc",
            session_file: Some(&vanished),
            session_dir: dir.path(),
            default_session_dir: dir.path(),
        };
        assert_eq!(format_resume_command(&missing, true), None);
    }

    /// The rendered line: Pi's label, chalk's dim pair, one trailing newline, no other decoration.
    #[test]
    fn the_hint_line_is_pis() {
        assert_eq!(
            resume_hint_line("cyrup --session abc"),
            "\x1b[2mTo resume this session:\x1b[22m cyrup --session abc\n"
        );
    }
}
