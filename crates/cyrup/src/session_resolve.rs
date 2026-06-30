//! Non-interactive session resolution (Pi `resolveSessionPath`/`createSessionManager`,
//! main.ts:163-189,254-350) — the closeable depth the bare `cli::session_target` lacked.
//!
//! `--session`/`--fork` accept a **partial UUID** (matched as a prefix against the cwd's session
//! listing, then a **global cross-project** scan) as well as a literal path; `--session-id` creates
//! its session **if missing by exact id**; a `--session` ref that lives in another project triggers a
//! plain-stdin "Fork this session into current directory? [y/N]" confirm (Pi `promptConfirm`,
//! main.ts:191-203); and an unresolvable ref / a non-interactive session whose stored cwd no longer
//! exists are diagnosed with Pi's exact messages + exit codes. The actual session-file listing seams
//! (`list_in_dir`/`list_all`) live in `cyrup-session`; this module is the pure orchestration over a
//! resolved set of [`SessionRef`]s so it is unit-testable without real session files on disk.

use std::path::{Path, PathBuf};

use cyrup_session_svc::{SessionInfo, SessionTarget};

/// The flag inputs the resolver reads (a narrow view of [`crate::Cli`] so the orchestration is
/// testable without constructing a full clap struct).
#[derive(Clone, Debug, Default)]
pub struct SessionFlags {
    pub fork: Option<String>,
    pub session: Option<String>,
    pub session_id: Option<String>,
    pub r#continue: bool,
    pub resume: bool,
    pub no_session: bool,
}

/// A lightweight `(id, path, cwd)` view of a listed session (Pi `SessionInfo`), the only fields the
/// partial-UUID / global resolution reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRef {
    pub id: String,
    pub path: PathBuf,
    pub cwd: String,
}

impl From<&SessionInfo> for SessionRef {
    fn from(info: &SessionInfo) -> Self {
        Self {
            id: info.id.as_str().to_string(),
            path: info.path.clone(),
            cwd: info.cwd.clone(),
        }
    }
}

/// How a `--session`/`--fork` argument resolved (Pi `ResolvedSession`, main.ts:142-147).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionLookup {
    /// A literal file path (the arg looked like a path).
    Path(PathBuf),
    /// An exact-or-prefix id match in the current project (carries the stored cwd for the
    /// missing-session-cwd guard).
    Local { path: PathBuf, cwd: String },
    /// An exact-or-prefix id match in a different project (carries that project's cwd).
    Global { path: PathBuf, cwd: String },
    /// No match anywhere.
    NotFound,
}

/// Resolve a session argument to a path (Pi `resolveSessionPath`, main.ts:163-189): a path-shaped arg
/// is taken verbatim (relative → absolute vs `cwd`); otherwise an **exact** then **prefix** id match
/// is tried against the local listing, then the global listing.
pub fn match_session_arg(
    arg: &str,
    cwd: &Path,
    locals: &[SessionRef],
    globals: &[SessionRef],
) -> SessionLookup {
    if arg.contains('/') || arg.contains('\\') || arg.ends_with(".jsonl") {
        let p = PathBuf::from(arg);
        let resolved = if p.is_absolute() { p } else { cwd.join(p) };
        return SessionLookup::Path(resolved);
    }
    if let Some(m) = exact_then_prefix(arg, locals) {
        return SessionLookup::Local { path: m.path.clone(), cwd: m.cwd.clone() };
    }
    if let Some(m) = exact_then_prefix(arg, globals) {
        return SessionLookup::Global { path: m.path.clone(), cwd: m.cwd.clone() };
    }
    SessionLookup::NotFound
}

/// Pi `list.find(s => s.id === arg) ?? list.find(s => s.id.startsWith(arg))` (main.ts:171-176): an
/// exact id wins, else the first prefix match.
fn exact_then_prefix<'a>(arg: &str, list: &'a [SessionRef]) -> Option<&'a SessionRef> {
    list.iter().find(|s| s.id == arg).or_else(|| list.iter().find(|s| s.id.starts_with(arg)))
}

/// What the orchestration decided (the bin maps this onto building a session or an exit).
#[derive(Debug)]
pub enum Outcome {
    /// Proceed to build the session with this target.
    Build(SessionTarget),
    /// Exit 0 (the user aborted a fork, or `--resume` selected nothing).
    ExitOk,
    /// Exit 1 (the `stderr` lines carry the `Error:`-prefixed diagnostic).
    ExitErr,
}

/// A resumed session whose stored working directory no longer exists, surfaced for the **interactive**
/// Continue/Cancel prompt (Pi `getMissingSessionCwdIssue` → `promptForMissingSessionCwd`,
/// main.ts:573-585 / session-cwd.ts). The non-interactive arm errors + exits 1 instead (it never sets
/// this); only an interactive run carries the issue back to the bin so it can run the selector.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissingSessionCwd {
    /// The session file to reopen against the chosen cwd if the user continues.
    pub session_file: PathBuf,
    /// The stored (now-missing) working directory.
    pub session_cwd: String,
    /// The current working directory offered as the `Continue` cwd (Pi `issue.fallbackCwd`).
    pub fallback_cwd: PathBuf,
}

/// The full resolution result: the [`Outcome`] plus any user-facing lines to emit (collected, not
/// printed, so tests can assert them and the bin owns the actual streams).
#[derive(Debug, Default)]
pub struct Resolution {
    pub outcome: Option<Outcome>,
    pub stdout: Vec<String>,
    pub stderr: Vec<String>,
    /// Set (interactive only) when a resumed session's stored cwd is gone: the bin runs the
    /// Continue/Cancel prompt and either reopens with the fallback cwd or exits 0.
    pub missing_cwd: Option<MissingSessionCwd>,
}

impl Resolution {
    fn build(mut self, target: SessionTarget) -> Self {
        self.outcome = Some(Outcome::Build(target));
        self
    }
    fn exit_err(mut self, message: impl Into<String>) -> Self {
        self.stderr.push(message.into());
        self.outcome = Some(Outcome::ExitErr);
        self
    }
}

/// Resolve the session target from the flags + the live listings, mirroring Pi `createSessionManager`
/// (main.ts:254-350). `confirm_fork` is invoked exactly once, only for the `--session`-found-in-another
/// -project case, and performs the plain-stdin `[y/N]` prompt (Pi `promptConfirm`); it returns the
/// user's yes/no. The `--resume` interactive picker is the outer-layer TUI host (it is not resolved
/// here — a bare `--resume` falls through to a fresh session, ledgered).
pub fn resolve_session_target(
    flags: &SessionFlags,
    cwd: &Path,
    locals: &[SessionRef],
    globals: &[SessionRef],
    non_interactive: bool,
    confirm_fork: &mut dyn FnMut() -> bool,
) -> Resolution {
    let r = Resolution::default();

    // `--no-session` is purely ephemeral; it ignores fork/session/continue resolution entirely (Pi
    // returns `inMemory` immediately, main.ts:265). A `--session-id` still seeds the ephemeral id.
    if flags.no_session {
        return match &flags.session_id {
            Some(id) => r.build(SessionTarget::CreateWithId(id.clone())),
            None => r.build(SessionTarget::New),
        };
    }

    // --fork: optionally guard a colliding `--session-id`, then resolve the source ref.
    if let Some(arg) = &flags.fork {
        if let Some(id) = &flags.session_id
            && locals.iter().any(|s| &s.id == id)
        {
            // Pi: `Session already exists with id '<id>'` + exit 1 (main.ts:290).
            return r.exit_err(format!("Session already exists with id '{id}'"));
        }
        let source = match match_session_arg(arg, cwd, locals, globals) {
            SessionLookup::Path(p) => p,
            SessionLookup::Local { path, .. } | SessionLookup::Global { path, .. } => path,
            // Pi: `No session found matching '<arg>'` + exit 1 (main.ts:303).
            SessionLookup::NotFound => {
                return r.exit_err(format!("No session found matching '{arg}'"));
            }
        };
        // A fork always anchors a fresh session at the *current* cwd, so the stored-cwd guard never
        // applies here (Pi `forkFrom(.., cwd, ..)`).
        return r.build(SessionTarget::Fork { source, id: flags.session_id.clone() });
    }

    // --session: open a local/path match; a global match prompts to fork into the cwd.
    if let Some(arg) = &flags.session {
        return match match_session_arg(arg, cwd, locals, globals) {
            SessionLookup::Path(p) => r.build(SessionTarget::Resume(p)),
            SessionLookup::Local { path, cwd: stored } => {
                // Non-interactive: a resumed session whose stored cwd is gone is a hard error (Pi
                // `MissingSessionCwdError`, main.ts:581-584). Interactive: surface the issue for the
                // bin's Continue/Cancel prompt (Pi `promptForMissingSessionCwd`, main.ts:575-580).
                resume_local(r, non_interactive, path, &stored, cwd)
            }
            SessionLookup::Global { path, cwd: foreign } => {
                let mut r = r;
                // Pi: `Session found in different project: <cwd>` (stdout, main.ts:317).
                r.stdout.push(format!("Session found in different project: {foreign}"));
                if confirm_fork() {
                    r.build(SessionTarget::Fork { source: path, id: None })
                } else {
                    // Pi: `Aborted.` + exit 0 (main.ts:321-323).
                    r.stdout.push("Aborted.".to_string());
                    r.outcome = Some(Outcome::ExitOk);
                    r
                }
            }
            SessionLookup::NotFound => r.exit_err(format!("No session found matching '{arg}'")),
        };
    }

    // --resume: the interactive picker is the outer-layer TUI host; fall through to a new session.
    if flags.r#continue {
        return r.build(SessionTarget::Continue);
    }

    // --session-id: open the exact local match, else create it (Pi main.ts:342-349).
    if let Some(id) = &flags.session_id {
        return match locals.iter().find(|s| &s.id == id) {
            Some(m) => resume_local(r, non_interactive, m.path.clone(), &m.cwd, cwd),
            None => r.build(SessionTarget::CreateWithId(id.clone())),
        };
    }

    r.build(SessionTarget::New)
}

/// Resolve a resumed **local** match (Pi: a `--session`/`--session-id` ref that opens an existing
/// session in the current project). Non-interactive: a stored-cwd-gone session is a hard error +
/// exit 1 (Pi `MissingSessionCwdError`). Interactive: the missing cwd is surfaced via
/// [`Resolution::missing_cwd`] (outcome left unset) so the bin runs the Continue/Cancel prompt; an
/// intact cwd resumes directly.
fn resume_local(
    r: Resolution,
    non_interactive: bool,
    path: PathBuf,
    stored: &str,
    cwd: &Path,
) -> Resolution {
    if non_interactive {
        if let Some(err) = missing_session_cwd_error(Some(path.as_path()), stored, cwd) {
            return r.exit_err(err);
        }
        return r.build(SessionTarget::Resume(path));
    }
    if session_cwd_is_missing(stored) {
        let mut r = r;
        r.missing_cwd = Some(MissingSessionCwd {
            session_file: path,
            session_cwd: stored.to_string(),
            fallback_cwd: cwd.to_path_buf(),
        });
        return r;
    }
    r.build(SessionTarget::Resume(path))
}

/// Whether a stored session cwd is non-empty yet no longer exists on disk (Pi
/// `getMissingSessionCwdIssue`: `!sessionCwd || existsSync(sessionCwd)` ⇒ no issue, session-cwd.ts:23).
pub fn session_cwd_is_missing(session_cwd: &str) -> bool {
    !session_cwd.is_empty() && !Path::new(session_cwd).exists()
}

/// Pi `formatMissingSessionCwdPrompt` (session-cwd.ts:40-42): the prompt body shown above the
/// interactive Continue/Cancel options.
pub fn format_missing_session_cwd_prompt(session_cwd: &str, fallback_cwd: &Path) -> String {
    format!(
        "cwd from session file does not exist\n{session_cwd}\n\ncontinue in current cwd\n{}",
        fallback_cwd.display()
    )
}

/// Pi `getMissingSessionCwdIssue` + `MissingSessionCwdError` (session-cwd.ts:14-42): a resumed session
/// whose stored working directory no longer exists is a hard error in non-interactive mode. Returns
/// the exact multi-line message when the issue applies, else `None`.
pub fn missing_session_cwd_error(
    session_file: Option<&Path>,
    session_cwd: &str,
    fallback_cwd: &Path,
) -> Option<String> {
    let file = session_file?;
    if session_cwd.is_empty() || Path::new(session_cwd).exists() {
        return None;
    }
    Some(format!(
        "Stored session working directory does not exist: {session_cwd}\nSession file: {}\nCurrent working directory: {}",
        file.display(),
        fallback_cwd.display()
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn sref(id: &str, path: &str, cwd: &str) -> SessionRef {
        SessionRef { id: id.into(), path: PathBuf::from(path), cwd: cwd.into() }
    }

    fn target_of(r: &Resolution) -> &SessionTarget {
        match r.outcome.as_ref().expect("an outcome") {
            Outcome::Build(t) => t,
            other => panic!("expected Build, got {other:?}"),
        }
    }

    #[test]
    fn path_shaped_args_resolve_verbatim() {
        let cwd = Path::new("/work");
        assert_eq!(
            match_session_arg("sub/s.jsonl", cwd, &[], &[]),
            SessionLookup::Path(PathBuf::from("/work/sub/s.jsonl"))
        );
        assert_eq!(
            match_session_arg("/abs/s.jsonl", cwd, &[], &[]),
            SessionLookup::Path(PathBuf::from("/abs/s.jsonl"))
        );
        // A bare `.jsonl` name is still path-shaped (Pi `endsWith(".jsonl")`).
        assert_eq!(
            match_session_arg("name.jsonl", cwd, &[], &[]),
            SessionLookup::Path(PathBuf::from("/work/name.jsonl"))
        );
    }

    #[test]
    fn partial_uuid_prefix_matches_local_then_global() {
        let cwd = Path::new("/work");
        let locals = vec![sref("abc12345-aaaa", "/work/s/abc.jsonl", "/work")];
        let globals = vec![
            sref("abc12345-aaaa", "/work/s/abc.jsonl", "/work"),
            sref("def67890-bbbb", "/other/s/def.jsonl", "/other"),
        ];
        // A unique local prefix → Local.
        assert_eq!(
            match_session_arg("abc12", cwd, &locals, &globals),
            SessionLookup::Local { path: PathBuf::from("/work/s/abc.jsonl"), cwd: "/work".into() }
        );
        // A prefix only present globally → Global with the foreign cwd.
        assert_eq!(
            match_session_arg("def67", cwd, &locals, &globals),
            SessionLookup::Global { path: PathBuf::from("/other/s/def.jsonl"), cwd: "/other".into() }
        );
        // Unmatched → NotFound.
        assert_eq!(match_session_arg("zzz", cwd, &locals, &globals), SessionLookup::NotFound);
    }

    #[test]
    fn exact_id_wins_over_a_prefix_collision() {
        let cwd = Path::new("/work");
        let locals = vec![
            sref("abcdef", "/work/s/prefix.jsonl", "/work"),
            sref("abc", "/work/s/exact.jsonl", "/work"),
        ];
        // "abc" matches the exact id, not the longer prefix-sharing one.
        assert_eq!(
            match_session_arg("abc", cwd, &locals, &[]),
            SessionLookup::Local { path: PathBuf::from("/work/s/exact.jsonl"), cwd: "/work".into() }
        );
    }

    #[test]
    fn session_id_creates_when_missing_and_opens_when_present() {
        let cwd = Path::new("/work");
        let flags = SessionFlags { session_id: Some("fresh".into()), ..Default::default() };
        let r = resolve_session_target(&flags, cwd, &[], &[], false, &mut || true);
        assert!(matches!(target_of(&r), SessionTarget::CreateWithId(id) if id == "fresh"));

        // Use an existing stored cwd so the (interactive) missing-cwd prompt does not engage.
        let here = std::env::current_dir().unwrap();
        let here = here.to_string_lossy().to_string();
        let locals = vec![sref("fresh", "/work/s/fresh.jsonl", &here)];
        let r = resolve_session_target(&flags, cwd, &locals, &[], false, &mut || true);
        assert!(matches!(target_of(&r), SessionTarget::Resume(p) if p == &PathBuf::from("/work/s/fresh.jsonl")));
    }

    #[test]
    fn fork_resolves_source_and_threads_the_id() {
        let cwd = Path::new("/work");
        let globals = vec![sref("src1", "/other/s/src.jsonl", "/other")];
        let flags = SessionFlags {
            fork: Some("src1".into()),
            session_id: Some("newid".into()),
            ..Default::default()
        };
        let r = resolve_session_target(&flags, cwd, &[], &globals, false, &mut || true);
        match target_of(&r) {
            SessionTarget::Fork { source, id } => {
                assert_eq!(source, &PathBuf::from("/other/s/src.jsonl"));
                assert_eq!(id.as_deref(), Some("newid"));
            }
            other => panic!("expected Fork, got {other:?}"),
        }
    }

    #[test]
    fn fork_into_existing_id_errors() {
        let cwd = Path::new("/work");
        let locals = vec![sref("taken", "/work/s/taken.jsonl", "/work")];
        let flags = SessionFlags {
            fork: Some("whatever".into()),
            session_id: Some("taken".into()),
            ..Default::default()
        };
        let r = resolve_session_target(&flags, cwd, &locals, &[], false, &mut || true);
        assert!(matches!(r.outcome, Some(Outcome::ExitErr)));
        assert!(r.stderr.iter().any(|l| l.contains("Session already exists with id 'taken'")));
    }

    #[test]
    fn fork_not_found_errors() {
        let flags = SessionFlags { fork: Some("nope".into()), ..Default::default() };
        let r = resolve_session_target(&flags, Path::new("/work"), &[], &[], false, &mut || true);
        assert!(matches!(r.outcome, Some(Outcome::ExitErr)));
        assert!(r.stderr.iter().any(|l| l.contains("No session found matching 'nope'")));
    }

    #[test]
    fn session_global_match_prompts_to_fork_and_honours_yes() {
        let cwd = Path::new("/work");
        let globals = vec![sref("g1", "/other/s/g.jsonl", "/other/project")];
        let flags = SessionFlags { session: Some("g1".into()), ..Default::default() };

        // yes → Fork; the "different project" line is surfaced.
        let mut yes = || true;
        let r = resolve_session_target(&flags, cwd, &[], &globals, false, &mut yes);
        assert!(matches!(target_of(&r), SessionTarget::Fork { id: None, .. }));
        assert!(r.stdout.iter().any(|l| l.contains("Session found in different project: /other/project")));

        // no → ExitOk + "Aborted.".
        let mut no = || false;
        let r = resolve_session_target(&flags, cwd, &[], &globals, false, &mut no);
        assert!(matches!(r.outcome, Some(Outcome::ExitOk)));
        assert!(r.stdout.iter().any(|l| l == "Aborted."));
    }

    #[test]
    fn session_local_match_resumes_without_prompting() {
        let cwd = Path::new("/work");
        let here = std::env::current_dir().unwrap();
        let here = here.to_string_lossy().to_string();
        let locals = vec![sref("loc", "/work/s/loc.jsonl", &here)];
        let flags = SessionFlags { session: Some("loc".into()), ..Default::default() };
        let mut confirm_called = false;
        let r = resolve_session_target(&flags, cwd, &locals, &[], false, &mut || {
            confirm_called = true;
            true
        });
        assert!(matches!(target_of(&r), SessionTarget::Resume(_)));
        assert!(!confirm_called, "a local match must not prompt");
    }

    #[test]
    fn continue_and_new_and_no_session() {
        let cwd = Path::new("/work");
        let cont = SessionFlags { r#continue: true, ..Default::default() };
        assert!(matches!(
            target_of(&resolve_session_target(&cont, cwd, &[], &[], false, &mut || true)),
            SessionTarget::Continue
        ));
        let new = SessionFlags::default();
        assert!(matches!(
            target_of(&resolve_session_target(&new, cwd, &[], &[], false, &mut || true)),
            SessionTarget::New
        ));
        // --no-session ignores resolution; with an id it seeds an ephemeral CreateWithId.
        let ns = SessionFlags { no_session: true, session_id: Some("x".into()), ..Default::default() };
        assert!(matches!(
            target_of(&resolve_session_target(&ns, cwd, &[], &[], false, &mut || true)),
            SessionTarget::CreateWithId(id) if id == "x"
        ));
    }

    #[test]
    fn interactive_missing_cwd_surfaces_issue_instead_of_erroring() {
        let cwd = Path::new("/work");
        let locals = vec![sref("loc", "/work/s/loc.jsonl", "/no/such/dir/xyzzy")];
        let flags = SessionFlags { session: Some("loc".into()), ..Default::default() };
        // Interactive (non_interactive=false): the stored cwd is gone → the issue is surfaced for the
        // prompt; no outcome is set so the bin can run the Continue/Cancel selector.
        let r = resolve_session_target(&flags, cwd, &locals, &[], false, &mut || true);
        assert!(r.outcome.is_none());
        let issue = r.missing_cwd.expect("a missing-cwd issue");
        assert_eq!(issue.session_cwd, "/no/such/dir/xyzzy");
        assert_eq!(issue.session_file, PathBuf::from("/work/s/loc.jsonl"));
        assert_eq!(issue.fallback_cwd, PathBuf::from("/work"));
        // Non-interactive: the same input is a hard error + exit 1, never a prompt.
        let r = resolve_session_target(&flags, cwd, &locals, &[], true, &mut || true);
        assert!(matches!(r.outcome, Some(Outcome::ExitErr)));
        assert!(r.missing_cwd.is_none());
    }

    #[test]
    fn interactive_present_cwd_resumes_without_a_prompt() {
        // A local match whose stored cwd still exists resumes directly even interactively (no issue).
        let here = std::env::current_dir().unwrap();
        let cwd = Path::new("/work");
        let locals = vec![sref("loc", "/work/s/loc.jsonl", &here.to_string_lossy())];
        let flags = SessionFlags { session: Some("loc".into()), ..Default::default() };
        let r = resolve_session_target(&flags, cwd, &locals, &[], false, &mut || true);
        assert!(r.missing_cwd.is_none());
        assert!(matches!(target_of(&r), SessionTarget::Resume(_)));
    }

    #[test]
    fn format_missing_cwd_prompt_matches_pi() {
        let s = format_missing_session_cwd_prompt("/gone", Path::new("/here"));
        assert!(s.contains("cwd from session file does not exist"));
        assert!(s.contains("/gone"));
        assert!(s.contains("continue in current cwd"));
        assert!(s.contains("/here"));
    }

    #[test]
    fn session_cwd_missing_predicate() {
        assert!(session_cwd_is_missing("/no/such/dir/xyzzy"));
        assert!(!session_cwd_is_missing(""));
        let here = std::env::current_dir().unwrap();
        assert!(!session_cwd_is_missing(&here.to_string_lossy()));
    }

    #[test]
    fn missing_cwd_error_only_when_stored_cwd_absent() {
        // An existing cwd (the test's own dir) → no issue.
        let here = std::env::current_dir().unwrap();
        assert!(missing_session_cwd_error(
            Some(Path::new("/s/a.jsonl")),
            &here.to_string_lossy(),
            Path::new("/fallback")
        )
        .is_none());
        // A definitely-absent cwd → the multi-line Pi message.
        let msg = missing_session_cwd_error(
            Some(Path::new("/s/a.jsonl")),
            "/no/such/dir/xyzzy",
            Path::new("/fallback"),
        )
        .expect("an issue");
        assert!(msg.contains("Stored session working directory does not exist: /no/such/dir/xyzzy"));
        assert!(msg.contains("Session file: /s/a.jsonl"));
        assert!(msg.contains("Current working directory: /fallback"));
        // No session file → never an issue (a fresh session).
        assert!(missing_session_cwd_error(None, "/no/such/dir", Path::new("/fallback")).is_none());
    }
}
