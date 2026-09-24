//! SUBA-110 — git "routing" variables are never inherited by a child that runs somewhere else.
//!
//! Port of pi-subagents `src/runs/shared/git-environment.ts` @v0.71.0 (#2437/#2440). When the
//! orchestrator itself runs under a git hook, `git --git-dir=…`, or anything else that exports
//! `GIT_DIR`/`GIT_INDEX_FILE`/`GIT_WORK_TREE`/…, a child that inherits them runs every `git` command
//! against the PARENT's repository and index instead of its own cwd or managed worktree. Upstream
//! strips them in exactly two places, and so does cyrup: the detached background runner
//! (`async-execution.ts:729`) and an external CLI that inherits the environment
//! (`external-cli-runner.ts:91`). An adapter's allowlisted environment is left alone on purpose
//! (`:90`): naming a variable there is a deliberate choice.

use std::ffi::{OsStr, OsString};

/// `GIT_ROUTING_VARIABLES` (`git-environment.ts:1-18`), upper-case.
const GIT_ROUTING_VARIABLES: [&str; 16] = [
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_CONFIG",
    "GIT_CONFIG_COUNT",
    "GIT_CONFIG_PARAMETERS",
    "GIT_DIR",
    "GIT_GRAFT_FILE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_NAMESPACE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_OBJECT_DIRECTORY",
    "GIT_PREFIX",
    "GIT_REPLACE_REF_BASE",
    "GIT_SHALLOW_FILE",
    "GIT_WORK_TREE",
];

/// `isGitRoutingVariable` (`:20-24`): the set, or `/^GIT_CONFIG_(?:KEY|VALUE)_\d+$/`, matched
/// case-insensitively because Windows environment names are (and Git for Windows reads them so).
/// A name that is not valid UTF-8 cannot be one of these.
#[must_use]
pub fn is_git_routing_variable(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    let upper = name.to_ascii_uppercase();
    if GIT_ROUTING_VARIABLES.contains(&upper.as_str()) {
        return true;
    }
    ["GIT_CONFIG_KEY_", "GIT_CONFIG_VALUE_"]
        .iter()
        .any(|prefix| {
            upper
                .strip_prefix(prefix)
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        })
}

/// `omitGitRoutingEnv` (`:26-29`) for a child that otherwise inherits: remove every routing name in
/// `inherited` (the orchestrator's variable names) from `command`. Call it BEFORE applying any
/// overlay, as upstream spreads the filtered `process.env` first and its own keys after.
pub fn omit_git_routing_env(
    command: &mut std::process::Command,
    inherited: impl IntoIterator<Item = OsString>,
) {
    for name in inherited {
        if is_git_routing_variable(&name) {
            command.env_remove(name);
        }
    }
}

/// [`omit_git_routing_env`] against this process's own environment.
pub fn omit_inherited_git_routing_env(command: &mut std::process::Command) {
    omit_git_routing_env(command, std::env::vars_os().map(|(name, _)| name));
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn the_upstream_set_and_pattern_match_case_insensitively() {
        for name in [
            "GIT_DIR",
            "git_dir",
            "Git_Index_File",
            "GIT_WORK_TREE",
            "GIT_CONFIG_PARAMETERS",
            "GIT_CONFIG_KEY_0",
            "git_config_value_12",
        ] {
            assert!(is_git_routing_variable(OsStr::new(name)), "{name}");
        }
        for name in [
            "GIT_AUTHOR_NAME",
            "GIT_SSH_COMMAND",
            "GIT_CONFIG_KEY_",
            "GIT_CONFIG_KEY_x",
            "GIT_CONFIG_VALUE_1a",
            "XGIT_DIR",
            "PATH",
        ] {
            assert!(!is_git_routing_variable(OsStr::new(name)), "{name}");
        }
    }

    #[test]
    fn only_routing_names_are_removed_and_an_overlay_applied_after_still_wins() {
        let mut command = std::process::Command::new("true");
        omit_git_routing_env(
            &mut command,
            [
                "GIT_DIR",
                "GIT_INDEX_FILE",
                "GIT_CONFIG_KEY_3",
                "GIT_AUTHOR_NAME",
                "HOME",
            ]
            .map(OsString::from),
        );
        command.env("GIT_WORK_TREE", "/overlay");
        let envs: Vec<(String, Option<String>)> = command
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert_eq!(
            envs,
            [
                ("GIT_CONFIG_KEY_3".to_owned(), None),
                ("GIT_DIR".to_owned(), None),
                ("GIT_INDEX_FILE".to_owned(), None),
                ("GIT_WORK_TREE".to_owned(), Some("/overlay".to_owned())),
            ],
            "removals only for routing names; an explicit overlay set afterwards is kept"
        );
    }

    /// End to end on a real child: with `GIT_DIR` pointing at another repository, a child spawned
    /// through the filter resolves the repository of its own cwd.
    #[cfg(unix)]
    #[test]
    fn a_filtered_child_runs_git_against_its_own_cwd() {
        let git = |dir: &std::path::Path, args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .env_remove("GIT_DIR")
                .output()
                .unwrap()
        };
        let parent = tempfile::tempdir().unwrap();
        let child = tempfile::tempdir().unwrap();
        for dir in [parent.path(), child.path()] {
            assert!(git(dir, &["init", "-q"]).status.success());
        }
        let parent_git_dir = parent.path().join(".git");

        let run = |filtered: bool| {
            let mut command = std::process::Command::new("git");
            command
                .args(["rev-parse", "--absolute-git-dir"])
                .current_dir(child.path())
                .env("GIT_DIR", &parent_git_dir);
            if filtered {
                // The inherited name list the orchestrator would see.
                omit_git_routing_env(&mut command, [OsString::from("GIT_DIR")]);
            }
            let out = command.output().unwrap();
            std::path::PathBuf::from(String::from_utf8(out.stdout).unwrap().trim())
        };
        assert_eq!(
            run(false).canonicalize().unwrap(),
            parent_git_dir.canonicalize().unwrap(),
            "precondition: an inherited GIT_DIR routes git to the parent repository"
        );
        assert_eq!(
            run(true).canonicalize().unwrap(),
            child.path().join(".git").canonicalize().unwrap(),
            "with the filter the child's git resolves its own cwd"
        );
    }
}
