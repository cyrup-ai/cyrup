//! The crate's SINGLE port of pi's `shared/utils.ts` path helpers.
//!
//! Every module that needs to know where the user's home directory, the agent directory or a
//! project's config directory is asks this module. There is exactly one home ladder
//! (`CYRUP_HOME` -> `HOME` -> [`std::env::temp_dir`]), one `getAgentDir()` and one
//! `getProjectConfigDir()` in the crate, so the same logical question can no longer get different
//! answers depending on which module asks it.
//!
//! # Why `CYRUP_HOME` comes first, and why that check is load-bearing
//!
//! `CYRUP_HOME` is the crate's sandbox lever: an integration test points it at a `TempDir`
//! precisely so no run artifact, mission pointer, settings file or worktree lands in the
//! developer's real home. Any resolver that skips the check silently escapes that sandbox —
//! `missions/store.rs` was once the one copy that did, and that omission alone leaked mission
//! pointers into a real `~/.cyrup` through nineteen correctly-sandboxed tests. Concentrating the
//! ladder here is what makes that class of bug structurally impossible rather than a comment
//! asserting that N private copies agree.

use std::path::{Path, PathBuf};

/// `os.homedir()` as this crate resolves it: `CYRUP_HOME` -> `HOME` -> [`std::env::temp_dir`].
///
/// Never returns an empty path: with neither variable set the process temp dir answers, so a
/// caller joining onto the result always gets an absolute path rather than a relative one rooted
/// at the process working directory.
#[must_use]
pub fn home_dir() -> PathBuf {
    std::env::var_os("CYRUP_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir)
}

/// pi `getAgentDir()` (`shared/utils.ts:95-100`) against an explicitly supplied home:
/// `$CYRUP_AGENT_DIR`/`$PI_CODING_AGENT_DIR` (with `~`/`~/` expansion against `home`) if set and
/// non-empty, else `<home>/.cyrup/agent`.
///
/// The injectable-home shape exists for the callers that already hold a home path (and for tests
/// that want to resolve against a temp home without moving the process environment);
/// [`agent_dir`] is the process-environment form.
#[must_use]
pub fn resolve_agent_dir(home: &Path) -> PathBuf {
    let configured = std::env::var("CYRUP_AGENT_DIR")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| {
            std::env::var("PI_CODING_AGENT_DIR")
                .ok()
                .filter(|v| !v.is_empty())
        });
    match configured {
        Some(v) if v == "~" => home.to_path_buf(),
        Some(v) if v.starts_with("~/") => home.join(v.get(2..).unwrap_or("")),
        Some(v) => PathBuf::from(v),
        None => home.join(".cyrup").join("agent"),
    }
}

/// pi `getAgentDir()` resolved against [`home_dir`] — the form nearly every caller wants.
#[must_use]
pub fn agent_dir() -> PathBuf {
    resolve_agent_dir(&home_dir())
}

/// pi `getProjectConfigDir(projectRoot)` (`shared/utils.ts:91-93`) — `<root>/.cyrup` (upstream
/// `<root>/.pi`), the same directory `cyrup_config::ConfigDirs::project_config_dir` names.
#[must_use]
pub fn project_config_dir(project_root: &Path) -> PathBuf {
    project_root.join(".cyrup")
}
