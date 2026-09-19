//! `git worktree list --porcelain`, parsed — pi `parseGitWorktreeList` (`:269-301` @`v0.68.0`).
//!
//! The parser is a pure `&str -> Vec<GitWorktreeRecord>` and is `pub(crate)` so it can be unit
//! tested directly. That is upstream's own shape too: `parseGitWorktreeList` is `export`ed and the
//! module keeps a `__testables` seam (`:176`).

use std::path::{Path, PathBuf};

use super::model::CleanupPlanError;

/// One row of `git worktree list --porcelain` — pi `GitWorktreeRecord` (`:97-102`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GitWorktreeRecord {
    /// The worktree root, exactly as git printed it.
    pub(crate) path: PathBuf,
    /// The `HEAD` line. `""` for a bare main worktree — pi keeps the record either way.
    pub(crate) head: String,
    /// The SHORT branch name: `refs/heads/` is stripped, because the manifest's cleanup task
    /// stores the short name and the two are compared directly (pi `:632`).
    pub(crate) branch: Option<String>,
    /// git's own prunable reason, when it reports one.
    pub(crate) prunable: Option<String>,
}

/// pi `parseGitWorktreeList` (`:269-301`), rule for rule.
///
/// * a blank line flushes the current record;
/// * `worktree <path>` flushes and starts a new record with `head: ""`;
/// * `HEAD <sha>`, `branch refs/heads/<name>`, `prunable <reason>` fill it;
/// * a bare `detached` line **clears** any branch;
/// * a record with an empty path is dropped, and the final record is flushed at EOF.
///
/// Every other line — `bare`, `locked`, `locked <reason>`, and anything before the first
/// `worktree ` — is IGNORED. That is upstream's behaviour and it is preserved deliberately: a
/// locked worktree gets no special treatment here and instead fails on a real gate later, where
/// the operator gets a reason rather than a silent omission.
#[must_use]
pub(crate) fn parse_git_worktree_list(raw: &str) -> Vec<GitWorktreeRecord> {
    let mut records: Vec<GitWorktreeRecord> = Vec::new();
    let mut current: Option<GitWorktreeRecord> = None;

    // pi `flush` (`:273-276`): a record with an empty path is dropped, not pushed.
    fn flush(current: &mut Option<GitWorktreeRecord>, records: &mut Vec<GitWorktreeRecord>) {
        if let Some(record) = current.take()
            && !record.path.as_os_str().is_empty()
        {
            records.push(record);
        }
    }

    for line in raw.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.trim().is_empty() {
            flush(&mut current, &mut records);
            continue;
        }
        if let Some(path) = line.strip_prefix("worktree ") {
            flush(&mut current, &mut records);
            current = Some(GitWorktreeRecord {
                path: PathBuf::from(path),
                head: String::new(),
                branch: None,
                prunable: None,
            });
            continue;
        }
        let Some(record) = current.as_mut() else {
            continue;
        };
        if let Some(head) = line.strip_prefix("HEAD ") {
            record.head = head.trim().to_string();
        } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
            record.branch = Some(branch.trim().to_string());
        } else if line == "detached" {
            record.branch = None;
        } else if let Some(reason) = line.strip_prefix("prunable ") {
            record.prunable = Some(reason.trim().to_string());
        }
    }
    flush(&mut current, &mut records);
    records
}

/// pi `listGitWorktrees` (`:295-299`).
///
/// # Errors
///
/// [`CleanupPlanError::Git`] carrying pi's `gitFailure` ladder verbatim.
pub(crate) async fn list_git_worktrees(
    repo_root: &Path,
) -> Result<Vec<GitWorktreeRecord>, CleanupPlanError> {
    let result = super::git::run(repo_root, &["worktree", "list", "--porcelain"]).await;
    let result = super::git::require_success(
        result,
        &format!("git -C {} worktree list --porcelain", repo_root.display()),
    )?;
    Ok(parse_git_worktree_list(&result))
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::unwrap_used
    )]

    use super::*;

    #[test]
    fn detached_clears_a_previously_seen_branch() {
        let records = parse_git_worktree_list(
            "worktree /tmp/a\nHEAD abc\nbranch refs/heads/lane-0\ndetached\n",
        );
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].branch, None);
        assert_eq!(records[0].head, "abc");
    }

    #[test]
    fn a_prunable_reason_is_captured_and_locked_is_ignored() {
        let records =
            parse_git_worktree_list("worktree /tmp/a\nHEAD abc\nlocked\nprunable gitdir gone\n");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].prunable.as_deref(), Some("gitdir gone"));
        assert_eq!(records[0].branch, None);
    }

    #[test]
    fn a_trailing_record_without_a_blank_line_still_flushes() {
        let records = parse_git_worktree_list(
            "worktree /repo\nHEAD aaa\nbranch refs/heads/main\n\nworktree /tmp/b\nHEAD bbb\nbranch refs/heads/lane-1",
        );
        assert_eq!(records.len(), 2);
        assert_eq!(records[1].path, PathBuf::from("/tmp/b"));
        assert_eq!(records[1].branch.as_deref(), Some("lane-1"));
    }

    #[test]
    fn lines_before_the_first_worktree_line_are_ignored() {
        let records = parse_git_worktree_list("HEAD deadbeef\nbare\nworktree /tmp/a\nHEAD abc\n");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].head, "abc");
    }

    #[test]
    fn the_branch_prefix_is_stripped_to_the_short_name() {
        let records =
            parse_git_worktree_list("worktree /tmp/a\nHEAD abc\nbranch refs/heads/feat/x\n");
        assert_eq!(records[0].branch.as_deref(), Some("feat/x"));
    }
}
