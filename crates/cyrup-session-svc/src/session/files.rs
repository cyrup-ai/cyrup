//! Session-file management on disk.
//!
//! Listing a project's sessions, and the delete/rename operations — including the trash-first
//! delete (with its per-platform argv) and the pi-compatible name hint truncation, which are free
//! functions so a caller without a live session can reach them.

use std::path::Path;

use crate::error::SessionServiceError;

use super::AgentSession;
use super::types::DeleteMethod;

impl AgentSession {
    /// The sessions root directory for this session (`agent_dir/sessions`, the layout default). The
    /// additive seam the `/resume` selector lists from.
    pub fn sessions_root(&self) -> std::path::PathBuf {
        self.services.agent_dir.join("sessions")
    }

    /// The directory THIS session's files live in — Pi `sessionManager.getSessionDir()`
    /// (session-manager.ts:999). Under an explicit `--session-dir`, or after resuming a file from
    /// somewhere else, this is NOT `<sessions_root>/--<encoded-cwd>--`.
    pub fn session_dir(&self) -> &Path {
        &self.services.session_dir
    }

    /// List the persisted sessions for this session, newest-first (Pi `SessionManager.list`,
    /// session-manager.ts:1638 → the `/resume` selector). Reads the session's OWN directory, exactly
    /// as Pi's picker does — `SessionManager.list(this.sessionManager.getCwd(),
    /// this.sessionManager.getSessionDir())` (interactive-mode.ts:4867) — so an explicit
    /// `--session-dir` (or a session resumed from elsewhere) lists the sessions actually next to
    /// this one rather than the cwd-encoded default dir, which may be empty or hold an unrelated set.
    ///
    /// A custom directory may pool SEVERAL projects' sessions in one flat dir, so Pi filters it by
    /// cwd — `filterCwd = sessionDir !== undefined && dir !== getDefaultSessionDirPath(cwd)`
    /// (session-manager.ts:1639-1643); the picker always passes `getSessionDir()`, so the predicate
    /// reduces to "not the cwd-encoded default". The default dir already isolates by cwd and so is
    /// never filtered. Same predicate the CLI `--resume` path computes (`session_list_cwd_filter`,
    /// crates/cyrup/src/main.rs:1132-1138). An absent/empty dir yields an empty list, never an error.
    pub fn list_sessions(&self) -> Vec<cyrup_session::listing::SessionInfo> {
        let dir = &self.services.session_dir;
        let default_dir =
            cyrup_session::SessionLayout::new(self.sessions_root(), self.services.cwd.clone()).dir();
        let cwd_filter = (*dir != default_dir).then_some(self.services.cwd.as_path());
        cyrup_session::listing::list_in_dir(dir, cwd_filter, None)
    }

    /// Delete a persisted session **file** by path (Pi `/resume` in-list delete → `app.session.delete`
    /// → `SessionManager.delete`, session-selector.ts:540). Additive seam for the TUI session selector:
    /// removes the JSONL from disk. Refuses to delete *this* session's own file (Pi guards the active
    /// session). An already-absent file is a no-op (idempotent), never an error.
    pub fn delete_session_file(&self, path: &Path) -> Result<DeleteMethod, SessionServiceError> {
        if let Some(active) = self.manager_path()
            && same_file(&active, path)
        {
            return Err(SessionServiceError::Io(
                "refusing to delete the active session".to_string(),
            ));
        }
        delete_session_file_at(path)
    }

    /// Set a persisted session's display **name** by path (Pi `/resume` in-list rename →
    /// `onRenameSession` → `SessionManager.setSessionName`, session-selector.ts:585). Additive seam:
    /// opens the target file, appends a `session_info` entry (the same persisted record
    /// [`Self::set_session_name`] writes for the active session), and lets the store flush. For the
    /// *active* session this routes through the live manager so the in-memory tree stays consistent.
    pub async fn rename_session_file(
        &self,
        path: &Path,
        name: &str,
    ) -> Result<(), SessionServiceError> {
        if let Some(active) = self.manager_path()
            && same_file(&active, path)
        {
            return self.set_session_name(name).await;
        }
        let mut mgr = cyrup_session::SessionManager::open(path)?;
        mgr.append_session_info(name)?;
        Ok(())
    }

    /// The on-disk path of this session's own JSONL, if the live manager exposes one (used to guard the
    /// active session from a `/resume` delete/rename).
    fn manager_path(&self) -> Option<std::path::PathBuf> {
        self.manager.try_lock().ok().and_then(|g| g.session_file().map(Path::to_path_buf))
    }
}

/// Delete a session JSONL, **trying the `trash` CLI first** — a literal port of pi's
/// `deleteSessionFile` (`modes/interactive/components/session-selector.ts:644-679` @v0.83.0,
/// byte-identical at v0.84.1). SEAM-063.
///
/// ```text
/// const trashArgs = sessionPath.startsWith("-") ? ["--", sessionPath] : [sessionPath];
/// const trashResult = spawnSync("trash", trashArgs, { encoding: "utf-8" });
/// if (trashResult.status === 0 || !existsSync(sessionPath)) return { ok: true, method: "trash" };
/// try { await unlink(sessionPath); return { ok: true, method: "unlink" }; }
/// catch (err) { … return { ok: false, method: "unlink", error }; }
/// ```
///
/// Four clauses are load-bearing and are reproduced exactly:
/// * the `--` guard for a leading-dash path (`:649`), so a session file named `-x.jsonl` is not
///   read as a `trash` option;
/// * success on exit-0 **or** the file having disappeared (`:666`) — some `trash` builds exit
///   non-zero while still moving the file;
/// * only then the permanent `unlink` (`:672`);
/// * the failure string carries BOTH the unlink message and pi's `trash: …` hint (`:675-678`),
///   truncated to pi's 200 characters and joined with pi's ` · `.
///
/// An already-absent file is success (cyrup's pre-existing idempotence, kept: pi's `!existsSync`
/// clause reaches the same verdict on that input).
pub fn delete_session_file_at(path: &Path) -> Result<DeleteMethod, SessionServiceError> {
    let mut cmd = std::process::Command::new("trash");
    cmd.args(trash_args(path));
    let trash = cmd.output();

    // Pi's `getTrashErrorHint()` (:651-663): the spawn error message and/or the FIRST line of
    // stderr, joined with " · " and sliced to 200 chars, prefixed `trash: `.
    let trash_hint = match &trash {
        Err(e) => Some(format!("trash: {e}")),
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let first = stderr.trim().lines().next().unwrap_or("").to_string();
            (!first.is_empty()).then(|| {
                let mut s = format!("trash: {first}");
                s.truncate(pi_hint_char_boundary(&s, 200));
                s
            })
        }
    };

    // Pi: `if (trashResult.status === 0 || !existsSync(sessionPath))` (:666).
    let trash_ok = matches!(&trash, Ok(out) if out.status.success());
    if trash_ok || !path.exists() {
        return Ok(DeleteMethod::Trash);
    }

    // Pi: the permanent fallback (:672-674).
    match std::fs::remove_file(path) {
        Ok(()) => Ok(DeleteMethod::Unlink),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(DeleteMethod::Unlink),
        Err(e) => Err(SessionServiceError::Io(match trash_hint {
            // Pi: `${unlinkError} (${trashErrorHint})` (:677).
            Some(hint) => format!("{e} ({hint})"),
            None => e.to_string(),
        })),
    }
}

/// Rename a persisted session **by path**, without a live session — the same two calls
/// [`AgentSession::rename_session_file`] makes for a non-active target (Pi `onRenameSession` →
/// `SessionManager.setSessionName`, `session-selector.ts:585`): open the JSONL, append a
/// `session_info` record.
///
/// Exists so the PRE-LAUNCH `--resume` picker can persist a rename it already accepts on screen; it
/// runs before any session services exist, and needs none. SEAM-062.
pub fn rename_session_file_at(path: &Path, name: &str) -> Result<(), SessionServiceError> {
    let mut mgr = cyrup_session::SessionManager::open(path)?;
    mgr.append_session_info(name)?;
    Ok(())
}

/// pi's `trashArgs` — `sessionPath.startsWith("-") ? ["--", sessionPath] : [sessionPath]`
/// (`modes/interactive/components/session-selector.ts:649` @v0.83.0).
///
/// The guard is load-bearing rather than defensive: a session file whose NAME begins with `-` (the
/// picker labels rows by their first message, and nothing stops a session id or a `--session-dir`
/// path producing one) would otherwise be parsed by `trash` as an option instead of a path.
/// Extracted so it is unit-testable without putting a stub `trash` on `PATH` — this crate forbids
/// `unsafe`, and `std::env::set_var` is `unsafe` under edition 2024.
pub(crate) fn trash_args(path: &Path) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = Vec::with_capacity(2);
    if path.to_string_lossy().starts_with('-') {
        args.push(std::ffi::OsString::from("--"));
    }
    args.push(path.as_os_str().to_os_string());
    args
}

/// The largest byte index `<= max` that is a char boundary — JS `String.slice(0, 200)` counts UTF-16
/// units and never panics, so the port must not either.
fn pi_hint_char_boundary(s: &str, max: usize) -> usize {
    if s.len() <= max {
        return s.len();
    }
    let mut idx = max;
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

/// `true` when two paths point at the same session file. Compares canonicalized paths when both
/// resolve (handling `..`/symlinks), else falls back to a lexical compare.
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => a == b,
    }
}
