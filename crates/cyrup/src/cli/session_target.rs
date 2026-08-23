use std::path::PathBuf;

use cyrup_session_svc::SessionTarget;

use super::args::Cli;

impl Cli {
    /// Which session the run targets. Resolution precedence mirrors Pi (main.ts:274-345):
    /// `--fork` ▷ `--session`/`--session-id` ▷ `--continue` ▷ new. A bare `--resume` is an
    /// interactive picker (resolved by the runtime), so it maps to `New` for the one-shot config.
    /// `base_session_dir` is the resolved sessions root used to turn a bare id into a file path.
    pub fn session_target(&self, base_session_dir: &std::path::Path) -> SessionTarget {
        if let Some(spec) = &self.fork {
            return SessionTarget::Resume(resolve_session_ref(spec, base_session_dir));
        }
        if let Some(spec) = self.session.as_ref().or(self.session_id.as_ref()) {
            return SessionTarget::Resume(resolve_session_ref(spec, base_session_dir));
        }
        if self.r#continue {
            return SessionTarget::Continue;
        }
        SessionTarget::New
    }

    /// Conflicting-session-flag diagnostics, a 1:1 port of Pi `validateForkFlags` (main.ts:205-219)
    /// and `validateSessionIdFlags` (main.ts:221-242). `--fork` conflicts with `--session`,
    /// `--continue`, `--resume`, `--no-session` (NOT `--session-id` — Pi forks into a new id);
    /// `--session-id` conflicts with `--session`, `--continue`, `--resume` and must pass
    /// [`assert_valid_session_id`]. The joined conflict list matches Pi's message exactly.
    pub fn validate_session_flags(&self) -> Result<(), String> {
        if self.fork.is_some() {
            let mut conflicts: Vec<&str> = Vec::new();
            if self.session.is_some() {
                conflicts.push("--session");
            }
            if self.r#continue {
                conflicts.push("--continue");
            }
            if self.resume {
                conflicts.push("--resume");
            }
            if self.no_session {
                conflicts.push("--no-session");
            }
            if !conflicts.is_empty() {
                return Err(format!(
                    "--fork cannot be combined with {}",
                    conflicts.join(", ")
                ));
            }
        }
        if let Some(id) = &self.session_id {
            let mut conflicts: Vec<&str> = Vec::new();
            if self.session.is_some() {
                conflicts.push("--session");
            }
            if self.r#continue {
                conflicts.push("--continue");
            }
            if self.resume {
                conflicts.push("--resume");
            }
            if !conflicts.is_empty() {
                return Err(format!(
                    "--session-id cannot be combined with {}",
                    conflicts.join(", ")
                ));
            }
            assert_valid_session_id(id)?;
        }
        Ok(())
    }
}

/// Resolve a `--session`/`--session-id`/`--fork` reference (a path or a bare id) to a session file
/// path (Pi `resolveSessionPath`, main.ts:297). An existing path is used verbatim; otherwise the id
/// is joined under `base_session_dir` with the `.jsonl` extension Pi uses for session files.
fn resolve_session_ref(spec: &str, base_session_dir: &std::path::Path) -> PathBuf {
    let as_path = PathBuf::from(spec);
    if as_path.exists() || spec.contains(std::path::MAIN_SEPARATOR) {
        return as_path;
    }
    let file = if spec.ends_with(".jsonl") {
        spec.to_string()
    } else {
        format!("{spec}.jsonl")
    };
    base_session_dir.join(file)
}

/// Validate a `--session-id` against Pi's id grammar (`assertValidSessionId`, session-manager.ts:207):
/// `^[A-Za-z0-9](?:[A-Za-z0-9._-]*[A-Za-z0-9])?$` — non-empty, only alphanumerics + `-`/`_`/`.`, and
/// alphanumeric at both ends. Returns Pi's exact error message on failure.
pub fn assert_valid_session_id(id: &str) -> Result<(), String> {
    let valid = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        && id.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && id
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric());
    if valid {
        Ok(())
    } else {
        Err("Session id must be non-empty, contain only alphanumeric characters, '-', '_', and '.', \
             and start and end with an alphanumeric character"
            .to_string())
    }
}
