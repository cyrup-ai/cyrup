//! SUBA-135 — the launch-cwd preflight, pi `preflightLaunchCwd` (`runs/shared/launch-cwd.ts:3-16`
//! @`pi-subagents` v0.71.0).
//!
//! Upstream checks the directory a child will run in BEFORE anything is launched — in the
//! foreground run (`execution.ts:1604`), in the async launch before the detached runner is spawned
//! (`async-execution.ts:649`), and in the runner before each step (`subagent-runner.ts:1061`) — so a
//! typo'd `cwd` is refused by name instead of surfacing as the OS error of a failed spawn (and, for
//! an async run, only after its receipt was returned).

use std::path::Path;

/// pi `preflightLaunchCwd(requestedCwd, effectiveCwd)`: `None` when `effective` is an existing
/// directory, else the refusal. `requested` is the cwd as the caller typed it; when it differs from
/// the resolved `effective` path the refusal names both (`(resolved from "<requested>")`).
///
/// [CYRUP-DELTA] The access-failure arm appends the OS error's own text, which is Rust's rendering
/// (`Permission denied (os error 13)`) rather than Node's (`EACCES: permission denied, stat …`).
#[must_use]
pub fn preflight_launch_cwd(requested: &str, effective: &Path) -> Option<String> {
    let effective_text = effective.display().to_string();
    let resolution = if requested == effective_text {
        String::new()
    } else {
        // `JSON.stringify(requestedCwd)`.
        format!(
            "\n(resolved from {})",
            serde_json::to_string(requested).unwrap_or_else(|_| format!("\"{requested}\""))
        )
    };
    match std::fs::metadata(effective) {
        Ok(metadata) if metadata.is_dir() => None,
        Ok(_) => Some(format!(
            "Subagent launch aborted: cwd is not a directory: {effective_text}{resolution}"
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(format!(
            "Subagent launch aborted: cwd does not exist: {effective_text}{resolution}"
        )),
        Err(error) => Some(format!(
            "Subagent launch aborted: cwd could not be accessed: {effective_text}{resolution}\n{error}"
        )),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn an_existing_directory_passes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().display().to_string();
        assert_eq!(preflight_launch_cwd(&path, dir.path()), None);
        assert_eq!(preflight_launch_cwd("elsewhere", dir.path()), None);
    }

    #[test]
    fn a_missing_cwd_is_refused_by_name_with_its_typed_spelling() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("nope");
        assert_eq!(
            preflight_launch_cwd("nope", &missing),
            Some(format!(
                "Subagent launch aborted: cwd does not exist: {}\n(resolved from \"nope\")",
                missing.display()
            ))
        );
        // Typed exactly as resolved: no resolution suffix.
        assert_eq!(
            preflight_launch_cwd(&missing.display().to_string(), &missing),
            Some(format!(
                "Subagent launch aborted: cwd does not exist: {}",
                missing.display()
            ))
        );
    }

    #[test]
    fn a_file_is_not_a_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("file.txt");
        std::fs::write(&file, "x").expect("write");
        assert_eq!(
            preflight_launch_cwd("file.txt", &file),
            Some(format!(
                "Subagent launch aborted: cwd is not a directory: {}\n(resolved from \"file.txt\")",
                file.display()
            ))
        );
    }

    #[test]
    fn a_path_through_a_file_could_not_be_accessed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("file.txt");
        std::fs::write(&file, "x").expect("write");
        let below = file.join("sub");
        let refusal = preflight_launch_cwd(&below.display().to_string(), &below)
            .expect("a path through a file is refused");
        assert!(
            refusal.starts_with(&format!(
                "Subagent launch aborted: cwd could not be accessed: {}\n",
                below.display()
            )),
            "{refusal}"
        );
    }
}
