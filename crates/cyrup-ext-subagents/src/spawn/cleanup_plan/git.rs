//! The plan builder's view of `git` — [`crate::spawn::worktree`]'s subprocess helpers, re-labelled
//! into this module's error type.
//!
//! This module exists so the plan builder never imports anything from `spawn::worktree` that
//! MUTATES. `spawn::worktree::cleanup_worktrees` is exactly the force-removing primitive a
//! cleanup PLAN must not call, and keeping the borrowed surface down to "run a git command and
//! tell me the exit status" is what makes that reviewable at a glance.

use std::path::Path;

use super::model::CleanupPlanError;
use crate::spawn::worktree::GitResult;

/// pi `runGit` (`:127-146`): never throws, hands back the raw result.
///
/// A spawn failure is folded into a `status: None` result carrying the OS error on `stderr`,
/// which is exactly pi's `{ status: null, error }` shape and keeps every caller's
/// "exit 0 or 1?" logic honest.
pub(crate) async fn run(cwd: &Path, args: &[&str]) -> GitResult {
    match crate::spawn::worktree::run_git(cwd, args).await {
        Ok(result) => result,
        Err(err) => GitResult {
            stdout: String::new(),
            stderr: err.to_string(),
            status: None,
        },
    }
}

/// pi `gitFailure` (`:145`): `stderr -> stdout -> "<command> failed"`, verbatim and unprefixed.
pub(crate) fn failure(result: &GitResult, command: &str) -> String {
    let stderr = result.stderr.trim();
    if !stderr.is_empty() {
        return stderr.to_string();
    }
    let stdout = result.stdout.trim();
    if !stdout.is_empty() {
        return stdout.to_string();
    }
    format!("{command} failed")
}

/// pi `runGitChecked` (`:147-151`): a non-zero exit aborts the whole plan.
///
/// # Errors
///
/// [`CleanupPlanError::Git`] carrying [`failure`]'s text.
pub(crate) fn require_success(
    result: GitResult,
    command: &str,
) -> Result<String, CleanupPlanError> {
    if result.status == Some(0) {
        return Ok(result.stdout.trim().to_string());
    }
    Err(CleanupPlanError::Git(failure(&result, command)))
}

/// `git rev-parse --verify <rev>^{commit}` — pi `resolveCommit` (`:563-567`) and
/// `resolveBranchTip` (`:569-573`), which are the same call with a different revision spelling.
///
/// Returns the resolved object name, or git's own failure text. Never an error type: an
/// unresolvable commit is a VERDICT on one worktree, not a failure of the plan.
pub(crate) async fn resolve_revision(repo_root: &Path, revision: &str) -> Result<String, String> {
    let spec = format!("{revision}^{{commit}}");
    let result = run(repo_root, &["rev-parse", "--verify", &spec]).await;
    if result.status == Some(0) {
        return Ok(result.stdout.trim().to_string());
    }
    Err(failure(
        &result,
        &format!("git -C {} rev-parse {revision}", repo_root.display()),
    ))
}
