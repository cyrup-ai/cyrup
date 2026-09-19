//! Git-worktree isolation for `worktree: true` parallel fan-out groups — a faithful port of
//! pi-subagents' `src/runs/shared/worktree.ts`.
//!
//! # What this module does (pi parity)
//!
//! For a fan-out group opted into worktree isolation, every concurrently-spawned child gets its
//! own dedicated working directory carved out of the shared repository via `git worktree add`
//! (never a shared cwd, which would let siblings stomp on each other's uncommitted changes). This
//! module reproduces pi's observable behavior exactly:
//!
//! - [`create_worktrees`] — verifies the shared tree is clean, resolves the repo-relative
//!   subdirectory (so each child's `agent_cwd` maps to the same subpath inside its worktree),
//!   creates one worktree/branch per task from `HEAD`, optionally symlinks `node_modules`, and runs
//!   an optional **per-worktree** setup hook. A failure at any point rolls back everything created
//!   so far and aborts with zero children spawned.
//! - [`diff_worktrees`] / [`capture_worktree_diff`] — the harvest side (C18): after the group runs,
//!   each worktree's work is captured as a per-task `.patch` plus a numstat summary, with
//!   hook-declared synthetic paths (and the `node_modules` symlink) removed *before* diffing so
//!   setup scaffolding never leaks into the captured patch.
//! - [`cleanup_worktrees`] — GATED removal of every worktree + branch, then `git worktree prune`.
//!   Called on the **success** path (after harvest) as well as on rollback (C18). A worktree that
//!   still holds work is removed only when the intent's evidence proves the work survives it: a
//!   validated captured patch the manifest already records, for the harvest path, or an explicit
//!   authority decision, for an operator discard.
//! - [`find_worktree_task_cwd_conflict`] — rejects a group only when a task's own `cwd` override
//!   points somewhere *other* than the shared cwd; a task cwd equal to the shared cwd is allowed.
//!
//! # Why this shells out to a real `git` subprocess, never a Rust git library
//!
//! `git worktree`/`git status`/`git diff` invocation deliberately shells out via subprocess —
//! consistent with this crate's subprocess-first design and exact stderr/stdout parity with the
//! real `git` CLI — rather than going through a Rust git library (`git worktree` has no mature
//! `gix` equivalent). This mirrors pi's own `spawnSync("git", ...)` usage.
//!
//! # Group-level wrappers over the pi-faithful primitives
//!
//! [`setup_worktree_group`] (plus [`WorktreeGroupConfig`]/[`WorktreeGroupPlan`]/
//! [`WorktreeAssignment`], [`HookSpec`]) is a thin group-shaped wrapper over [`create_worktrees`],
//! and is what `spawn::chain_graph::assign_worktree_cwds` calls while the crate converges on pi's
//! `create_worktrees`/`diff_worktrees`/`cleanup_worktrees` contract.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use crate::error::SubagentError;

/// Default bound on the optional setup hook's total runtime, in milliseconds (pi's
/// `DEFAULT_WORKTREE_SETUP_HOOK_TIMEOUT_MS`, 30000ms).
pub const DEFAULT_WORKTREE_SETUP_HOOK_TIMEOUT_MS: u64 = 30_000;

/// Environment variable naming the base directory new worktrees are created under when neither a
/// per-call `base_dir` nor an explicit config value is supplied (pi's `PI_SUBAGENTS_WORKTREE_DIR`,
/// cyrup equivalent). When unset, the base directory defaults to [`std::env::temp_dir`].
pub const WORKTREE_DIR_ENV: &str = "CYRUP_SUBAGENTS_WORKTREE_DIR";

/// pi `MACHINE_DIFF_OPTIONS` (`runs/shared/worktree.ts:23` @v0.68.0) — the flags that make
/// `git diff` output a function of the REPOSITORY rather than of whoever's config it ran under.
///
/// Every one of them closes a way a captured patch can stop describing the worktree it came from:
/// `--no-color` (a `color.diff = always` config injects ANSI escapes into the patch body),
/// `--no-ext-diff` / `--no-textconv` (a `.gitattributes` `diff=` driver renders the file instead
/// of diffing it, so the patch holds a DESCRIPTION of a change rather than the change),
/// `--default-prefix` and `--no-relative` (`diff.mnemonicPrefix`, `diff.noPrefix` and
/// `diff.relative` move the `a/`…`b/` prefixes `git apply -p1` expects), `--line-prefix=` (clears
/// any configured prefix that would be prepended to every output line).
///
/// This matters here and not merely upstream because a harvested worktree is REMOVED right after
/// capture ([`crate::spawn::chain_graph`]'s `publish_worktree_handoff`), so the patch is all that
/// survives — and because [`validate_worktree_patch_represents_current_worktree`] compares a
/// stored patch against a freshly re-captured one byte for byte, which is only meaningful if both
/// captures are config-independent.
const MACHINE_DIFF_OPTIONS: &[&str] = &[
    "--no-color",
    "--no-ext-diff",
    "--no-textconv",
    "--default-prefix",
    "--line-prefix=",
    "--no-relative",
];

/// pi `MACHINE_PATCH_OPTIONS` (`worktree.ts:24`) — [`MACHINE_DIFF_OPTIONS`] plus `--binary`.
///
/// `--binary` is the difference between a patch and a note saying a patch was not taken: without
/// it git emits `Binary files a/x.png and b/x.png differ`, which `git apply` cannot apply, so a
/// child's binary output would be unrecoverable the moment its worktree is removed. Used for the
/// stored `.patch` and for every re-capture compared against it; the `--stat`/`--numstat`
/// summaries use the plain [`MACHINE_DIFF_OPTIONS`], as upstream does.
const MACHINE_PATCH_OPTIONS: &[&str] = &[
    "--no-color",
    "--no-ext-diff",
    "--no-textconv",
    "--default-prefix",
    "--line-prefix=",
    "--no-relative",
    "--binary",
];

// =================================================================================================
// Data model (pi worktree.ts interfaces)
// =================================================================================================

/// The result of [`create_worktrees`]: the resolved repo toplevel, every per-task worktree, and
/// the common base commit the group was cut from (pi `WorktreeSetup`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeSetup {
    /// The repository toplevel (`git rev-parse --show-toplevel`) every worktree hangs off of.
    pub cwd: PathBuf,
    /// One entry per task, in task order.
    pub worktrees: Vec<WorktreeInfo>,
    /// The commit (`git rev-parse HEAD` at setup time) diffs are taken against.
    pub base_commit: String,
}

/// One concurrently-spawned child's dedicated worktree (pi `WorktreeInfo`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeInfo {
    /// The worktree root directory (`git worktree add <path>`).
    pub path: PathBuf,
    /// The directory the child should actually use as its `cwd` — `path` joined with the same
    /// repo-relative subdirectory the caller's shared cwd sat in (so a child launched from
    /// `<repo>/packages/app` runs in `<worktree>/packages/app`).
    pub agent_cwd: PathBuf,
    /// The branch `git worktree add -b <branch> HEAD` created.
    pub branch: String,
    /// This task's 0-based position within the group.
    pub index: u32,
    /// Whether a `node_modules` symlink was created into this worktree.
    pub node_modules_linked: bool,
    /// Worktree-relative paths (e.g. `node_modules`, hook-declared scaffolding) excluded from this
    /// worktree's captured diff.
    pub synthetic_paths: Vec<String>,
}

/// A per-task captured diff (pi `WorktreeDiff`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeDiff {
    /// The task's 0-based index.
    pub index: u32,
    /// The agent name (or `task-<n>` fallback) this diff belongs to.
    pub agent: String,
    /// The worktree's branch.
    pub branch: String,
    /// `git diff --cached --stat <base>` output (trimmed).
    pub diff_stat: String,
    /// Files changed, from `--numstat`.
    pub files_changed: u64,
    /// Total insertions, from `--numstat`.
    pub insertions: u64,
    /// Total deletions, from `--numstat`.
    pub deletions: u64,
    /// The `.patch` file this diff was written to.
    pub patch_path: PathBuf,
    /// Why the capture failed, when it failed — pi `WorktreeDiff.error` (`worktree.ts:79`).
    ///
    /// A failed capture still produces a row (the harvest must not lose a task), but it must be
    /// distinguishable from a genuinely empty one: the durable `.patch` that row names is a
    /// zero-byte placeholder, not evidence of "nothing changed". This field is what
    /// [`cleanup_worktrees`]'s preserve gate reads to refuse removing the worktree whose capture
    /// failed, and what [`crate::handoff::Patch::error`] carries into the manifest.
    pub error: Option<String>,
}

/// A task whose explicit `cwd` override conflicts with worktree isolation (pi
/// `WorktreeTaskCwdConflict`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeTaskCwdConflict {
    /// The offending task's index.
    pub index: usize,
    /// The offending task's agent name.
    pub agent: String,
    /// The offending (raw) cwd value the task declared.
    pub cwd: String,
}

/// Configuration for the optional per-worktree setup hook (pi `WorktreeSetupHookConfig`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorktreeSetupHookConfig {
    /// Absolute or repo-relative path to the hook executable (a bare command name is rejected).
    pub hook_path: String,
    /// Optional per-hook timeout override, in milliseconds.
    pub timeout_ms: Option<u64>,
}

/// Options accepted by [`create_worktrees`] (pi `CreateWorktreesOptions`).
#[derive(Debug, Clone, Default)]
pub struct CreateWorktreesOptions {
    /// Per-task agent names (used to enrich the hook payload); indexed by task position.
    pub agents: Option<Vec<String>>,
    /// The optional per-worktree setup hook.
    pub setup_hook: Option<WorktreeSetupHookConfig>,
    /// Base directory override; see [`WORKTREE_DIR_ENV`] and [`std::env::temp_dir`] for the
    /// resolution order.
    pub base_dir: Option<String>,
}

/// The resolved, validated setup hook (pi `ResolvedWorktreeSetupHook`).
#[derive(Debug, Clone)]
struct ResolvedWorktreeSetupHook {
    hook_path: PathBuf,
    timeout_ms: u64,
}

/// The JSON payload written to a per-worktree setup hook's stdin (pi `WorktreeSetupHookInput`).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct WorktreeSetupHookInput<'a> {
    version: u8,
    repo_root: &'a Path,
    worktree_path: &'a Path,
    agent_cwd: &'a Path,
    branch: &'a str,
    index: u32,
    run_id: &'a str,
    base_commit: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent: Option<&'a str>,
}

/// The JSON payload expected back from a setup hook's stdout (pi `WorktreeSetupHookOutput`).
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorktreeSetupHookOutput {
    #[serde(default)]
    synthetic_paths: Option<serde_json::Value>,
}

/// The resolved repository state (pi `RepoState`).
struct RepoState {
    toplevel: PathBuf,
    cwd_relative: String,
    base_commit: String,
}

/// The raw result of one `git` invocation (pi `GitResult`, `worktree-cleanup-plan.ts:90-95`).
///
/// `pub(crate)` since the cleanup-plan builder ([`crate::spawn::cleanup_plan`]) classifies a
/// worktree by the exit STATUS of probe commands (`git diff --quiet` is 0-or-1, `git merge-base
/// --is-ancestor` is 0-or-1), so it needs the raw result rather than
/// [`run_git_checked`]'s collapse-to-error.
pub(crate) struct GitResult {
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) status: Option<i32>,
}

// =================================================================================================
// git subprocess helpers (pi runGit / runGitChecked)
// =================================================================================================

pub(crate) async fn run_git(cwd: &Path, args: &[&str]) -> Result<GitResult, SubagentError> {
    run_git_env(cwd, args, &[]).await
}

/// [`run_git`] with extra environment variables for the child process only.
///
/// Required by [`crate::spawn::cleanup_plan`]'s patch re-capture, which reproduces pi
/// `currentWorktreePatch` (`worktree.ts:303-327`): it stages into a TEMPORARY `GIT_INDEX_FILE`
/// precisely so a planning operation never touches the worktree's real index. Without this
/// variant a transliteration of that function would `git add -A` into the live index — a
/// mutation, during an operation whose whole contract is that it mutates nothing but the plan
/// file. The env seam is therefore a SAFETY requirement, not a convenience.
pub(crate) async fn run_git_env(
    cwd: &Path,
    args: &[&str],
    env: &[(&str, &Path)],
) -> Result<GitResult, SubagentError> {
    let mut command = Command::new("git");
    command.args(args).current_dir(cwd);
    for (key, value) in env {
        command.env(key, value);
    }
    let output = command.output().await.map_err(SubagentError::Spawn)?;
    Ok(GitResult {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        status: output.status.code(),
    })
}

pub(crate) async fn run_git_checked(cwd: &Path, args: &[&str]) -> Result<String, SubagentError> {
    let result = run_git(cwd, args).await?;
    if result.status != Some(0) {
        let command = format!("git -C {} {}", cwd.display(), args.join(" "));
        let stderr = result.stderr.trim();
        let stdout = result.stdout.trim();
        let message = if !stderr.is_empty() {
            stderr.to_string()
        } else if !stdout.is_empty() {
            stdout.to_string()
        } else {
            format!("{command} failed")
        };
        return Err(SubagentError::WorktreeSetup(message));
    }
    Ok(result.stdout)
}

// =================================================================================================
// Path helpers
// =================================================================================================

/// Lexically normalize a path (resolve `.`/`..` textually, without touching the filesystem) —
/// the equivalent of Node's `path.resolve`/`path.normalize` for the containment checks below,
/// which must work on worktree paths that may not exist yet.
pub(crate) fn lexical_normalize(base: &Path, relative: &Path) -> PathBuf {
    let joined = base.join(relative);
    let mut out: Vec<std::path::Component<'_>> = Vec::new();
    for comp in joined.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => match out.last() {
                Some(std::path::Component::Normal(_)) => {
                    out.pop();
                }
                _ => out.push(comp),
            },
            other => out.push(other),
        }
    }
    out.iter().map(|component| component.as_os_str()).collect()
}

/// pi `normalizeComparableCwd`: absolute-resolve then realpath, falling back to the unresolved
/// absolute path when realpath resolution is unavailable.
pub(crate) fn normalize_comparable_cwd(cwd: &Path) -> PathBuf {
    let resolved = std::path::absolute(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    std::fs::canonicalize(&resolved).unwrap_or(resolved)
}

/// pi `safePatchAgentName`: replace every character outside `[\w.-]` with `_`.
fn safe_patch_agent_name(agent: &str) -> String {
    agent
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// pi `buildWorktreeBranch` (cyrup-prefixed): `cyrup-parallel-<runId>-<index>`.
fn build_worktree_branch(run_id: &str, index: u32) -> String {
    format!("cyrup-parallel-{run_id}-{index}")
}

/// pi `buildWorktreePath` (cyrup-prefixed): `<baseDir>/cyrup-worktree-<runId>-<index>`.
fn build_worktree_path(base_dir: &Path, run_id: &str, index: u32) -> PathBuf {
    base_dir.join(format!("cyrup-worktree-{run_id}-{index}"))
}

/// pi `resolveWorktreeBaseDir`, WITHOUT the `mkdir -p`.
///
/// Split out of [`resolve_worktree_base_dir`] so that a READER can ask "where does this build put
/// managed worktrees?" without the answer having the side effect of creating that directory.
/// [`crate::spawn::cleanup_plan`] is that reader: building a cleanup plan must mutate nothing but
/// the plan file, and a create-side helper that mkdir's would break that contract the moment an
/// operator configured `subagents.worktreeBaseDir`.
///
/// # Errors
///
/// [`SubagentError::WorktreeSetup`] when a configured base directory trims to empty (pi `:221`).
pub(crate) fn resolve_worktree_base_dir_path(
    configured_base_dir: Option<&str>,
    repo_root: &Path,
) -> Result<PathBuf, SubagentError> {
    Ok(
        configured_worktree_base_dir(configured_base_dir, repo_root)?
            .unwrap_or_else(std::env::temp_dir),
    )
}

/// The CONFIGURED base directory, or `None` when neither the config key nor
/// [`WORKTREE_DIR_ENV`] names one. Separated from the `temp_dir()` fallback because the two
/// have different ownership: cyrup created the configured directory and may create it again,
/// while the system temp directory is not cyrup's to make.
fn configured_worktree_base_dir(
    configured_base_dir: Option<&str>,
    repo_root: &Path,
) -> Result<Option<PathBuf>, SubagentError> {
    let raw = configured_base_dir
        .map(str::to_string)
        .or_else(|| std::env::var(WORKTREE_DIR_ENV).ok());
    let Some(raw) = raw else {
        return Ok(None);
    };

    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(SubagentError::WorktreeSetup(
            "worktree base directory cannot be empty".to_string(),
        ));
    }

    let expanded: PathBuf = trimmed.strip_prefix("~/").map_or_else(
        || PathBuf::from(trimmed),
        |rest| crate::paths::home_dir().join(rest),
    );
    Ok(Some(if expanded.is_absolute() {
        expanded
    } else {
        repo_root.join(expanded)
    }))
}

/// pi `resolveWorktreeBaseDir`: [`resolve_worktree_base_dir_path`] plus the `mkdir -p` the CREATE
/// side needs before `git worktree add` can land a leaf under it.
fn resolve_worktree_base_dir(
    configured_base_dir: Option<&str>,
    repo_root: &Path,
) -> Result<PathBuf, SubagentError> {
    let Some(resolved) = configured_worktree_base_dir(configured_base_dir, repo_root)? else {
        return Ok(std::env::temp_dir());
    };
    std::fs::create_dir_all(&resolved).map_err(|err| {
        SubagentError::WorktreeSetup(format!(
            "failed to create worktree base directory {}: {err}",
            resolved.display()
        ))
    })?;
    Ok(resolved)
}

/// pi `resolveRepoCwdRelative`: verify `cwd` is inside a work tree, then return its normalized
/// repo-relative prefix (`""` at the repo root).
async fn resolve_repo_cwd_relative(cwd: &Path) -> Result<String, SubagentError> {
    let repo_check = run_git(cwd, &["rev-parse", "--is-inside-work-tree"]).await?;
    if repo_check.status != Some(0) || repo_check.stdout.trim() != "true" {
        return Err(SubagentError::WorktreeSetup(
            "worktree isolation requires a git repository".to_string(),
        ));
    }
    let raw_prefix = run_git_checked(cwd, &["rev-parse", "--show-prefix"]).await?;
    let stripped = raw_prefix.trim().trim_end_matches(['/', '\\']);
    if stripped.is_empty() {
        return Ok(String::new());
    }
    let normalized = lexical_normalize(Path::new(""), Path::new(stripped));
    let normalized = normalized.to_string_lossy().into_owned();
    Ok(if normalized == "." {
        String::new()
    } else {
        normalized
    })
}

/// pi `resolveExpectedWorktreeAgentCwd`: compute (without creating anything) the `agent_cwd` a
/// task at `index` would receive, for previewing/reporting a worktree layout up front.
///
/// # Errors
///
/// Returns [`SubagentError::WorktreeSetup`] if `cwd` is not inside a git work tree or the base
/// directory cannot be resolved.
pub async fn resolve_expected_worktree_agent_cwd(
    cwd: &Path,
    run_id: &str,
    index: u32,
    base_dir: Option<&str>,
) -> Result<PathBuf, SubagentError> {
    let cwd_relative = resolve_repo_cwd_relative(cwd).await?;
    let repo_root = PathBuf::from(
        run_git_checked(cwd, &["rev-parse", "--show-toplevel"])
            .await?
            .trim(),
    );
    let base = resolve_worktree_base_dir(base_dir, &repo_root)?;
    let worktree_path = build_worktree_path(&base, run_id, index);
    Ok(if cwd_relative.is_empty() {
        worktree_path
    } else {
        worktree_path.join(&cwd_relative)
    })
}

/// pi `resolveRepoState`.
async fn resolve_repo_state(cwd: &Path) -> Result<RepoState, SubagentError> {
    let cwd_relative = resolve_repo_cwd_relative(cwd).await?;
    let toplevel = PathBuf::from(
        run_git_checked(cwd, &["rev-parse", "--show-toplevel"])
            .await?
            .trim(),
    );

    let status = run_git_checked(&toplevel, &["status", "--porcelain"]).await?;
    if !status.trim().is_empty() {
        return Err(SubagentError::WorktreeSetup(
            "worktree isolation requires a clean git working tree. Commit or stash changes first."
                .to_string(),
        ));
    }

    let base_commit = run_git_checked(&toplevel, &["rev-parse", "HEAD"])
        .await?
        .trim()
        .to_string();
    Ok(RepoState {
        toplevel,
        cwd_relative,
        base_commit,
    })
}

// =================================================================================================
// Task cwd conflict detection (pi findWorktreeTaskCwdConflict / formatWorktreeTaskCwdConflict)
// =================================================================================================

/// pi `findWorktreeTaskCwdConflict`: return the first task whose `cwd` override resolves to a
/// directory *other* than the shared cwd. A task with no `cwd`, or a `cwd` equal to the shared cwd
/// (including a relative `.`), is allowed.
///
/// `tasks` is a slice of `(agent, cwd)` pairs, `cwd` being the task's optional raw override.
#[must_use]
pub fn find_worktree_task_cwd_conflict(
    tasks: &[(&str, Option<&str>)],
    shared_cwd: &Path,
) -> Option<WorktreeTaskCwdConflict> {
    let normalized_shared = normalize_comparable_cwd(shared_cwd);
    for (index, (agent, cwd)) in tasks.iter().enumerate() {
        let Some(cwd) = cwd else { continue };
        let task_cwd = if Path::new(cwd).is_absolute() {
            PathBuf::from(cwd)
        } else {
            std::path::absolute(shared_cwd.join(cwd)).unwrap_or_else(|_| shared_cwd.join(cwd))
        };
        if normalize_comparable_cwd(&task_cwd) == normalized_shared {
            continue;
        }
        return Some(WorktreeTaskCwdConflict {
            index,
            agent: (*agent).to_string(),
            cwd: (*cwd).to_string(),
        });
    }
    None
}

/// pi `formatWorktreeTaskCwdConflict`.
#[must_use]
pub fn format_worktree_task_cwd_conflict(
    conflict: &WorktreeTaskCwdConflict,
    shared_cwd: &Path,
) -> String {
    format!(
        "worktree isolation uses the shared cwd ({}); task {} ({}) sets cwd to {}. Remove \
         task-level cwd overrides or disable worktree.",
        shared_cwd.display(),
        conflict.index + 1,
        conflict.agent,
        conflict.cwd
    )
}

// =================================================================================================
// node_modules symlinking (pi linkNodeModulesIfPresent)
// =================================================================================================

fn link_node_modules_if_present(toplevel: &Path, worktree_path: &Path) -> bool {
    let node_modules_path = toplevel.join("node_modules");
    let node_modules_link_path = worktree_path.join("node_modules");
    if !node_modules_path.exists() || node_modules_link_path.symlink_metadata().is_ok() {
        return false;
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&node_modules_path, &node_modules_link_path).is_ok()
    }
    #[cfg(not(unix))]
    {
        std::os::windows::fs::symlink_dir(&node_modules_path, &node_modules_link_path).is_ok()
    }
}

// =================================================================================================
// Setup hook resolution + invocation (pi resolveWorktreeSetupHook / runWorktreeSetupHook)
// =================================================================================================

fn parse_hook_timeout(timeout_ms: Option<u64>) -> Result<u64, SubagentError> {
    match timeout_ms {
        None => Ok(DEFAULT_WORKTREE_SETUP_HOOK_TIMEOUT_MS),
        Some(0) => Err(SubagentError::WorktreeSetup(
            "worktree setup hook timeout must be an integer greater than 0".to_string(),
        )),
        Some(value) => Ok(value),
    }
}

/// pi `resolveWorktreeSetupHook`: expand `~/`, require an absolute or repo-relative path (reject a
/// bare command name), and require the resolved path to be an existing file.
fn resolve_worktree_setup_hook(
    repo_root: &Path,
    config: Option<&WorktreeSetupHookConfig>,
) -> Result<Option<ResolvedWorktreeSetupHook>, SubagentError> {
    let Some(config) = config else {
        return Ok(None);
    };
    let hook_path = config.hook_path.trim();
    if hook_path.is_empty() {
        return Err(SubagentError::WorktreeSetup(
            "worktree setup hook path cannot be empty".to_string(),
        ));
    }

    let expanded: PathBuf = hook_path.strip_prefix("~/").map_or_else(
        || PathBuf::from(hook_path),
        |rest| crate::paths::home_dir().join(rest),
    );

    let resolved_path = if expanded.is_absolute() {
        expanded
    } else if hook_path.contains('/') || hook_path.contains('\\') {
        repo_root.join(&expanded)
    } else {
        return Err(SubagentError::WorktreeSetup(
            "worktree setup hook must be an absolute path or a repo-relative path".to_string(),
        ));
    };

    let metadata = std::fs::metadata(&resolved_path).map_err(|_| {
        SubagentError::WorktreeSetup(format!(
            "worktree setup hook not found: {}",
            resolved_path.display()
        ))
    })?;
    if metadata.is_dir() {
        return Err(SubagentError::WorktreeSetup(format!(
            "worktree setup hook must be a file, got directory: {}",
            resolved_path.display()
        )));
    }

    Ok(Some(ResolvedWorktreeSetupHook {
        hook_path: resolved_path,
        timeout_ms: parse_hook_timeout(config.timeout_ms)?,
    }))
}

/// pi `normalizeSyntheticPath`: a hook-declared path must be relative, non-empty, and contained
/// within (but not equal to) the worktree root.
fn normalize_synthetic_path(worktree_path: &Path, raw_path: &str) -> Result<String, SubagentError> {
    let trimmed = raw_path.trim();
    if trimmed.is_empty() {
        return Err(SubagentError::WorktreeSetup(
            "synthetic path cannot be empty".to_string(),
        ));
    }
    if Path::new(trimmed).is_absolute() {
        return Err(SubagentError::WorktreeSetup(format!(
            "synthetic path must be relative: {raw_path}"
        )));
    }
    let resolved = lexical_normalize(worktree_path, Path::new(trimmed));
    let relative = resolved.strip_prefix(worktree_path).ok();
    match relative {
        None => Err(SubagentError::WorktreeSetup(format!(
            "synthetic path escapes the worktree root: {raw_path}"
        ))),
        Some(rel) if rel.as_os_str().is_empty() => Err(SubagentError::WorktreeSetup(format!(
            "synthetic path cannot target the worktree root: {raw_path}"
        ))),
        Some(rel) => Ok(rel.to_string_lossy().into_owned()),
    }
}

/// pi `hasTrackedEntries`: `git ls-files -- <relativePath>` reports a tracked match.
async fn has_tracked_entries(worktree_path: &Path, relative_path: &str) -> bool {
    match run_git(worktree_path, &["ls-files", "--", relative_path]).await {
        Ok(result) => result.status == Some(0) && !result.stdout.trim().is_empty(),
        Err(_) => false,
    }
}

/// pi `parseWorktreeSetupHookOutput` + the `syntheticPaths` validation loop of
/// `runWorktreeSetupHook`.
async fn parse_and_validate_hook_output(
    worktree_path: &Path,
    raw_stdout: &str,
) -> Result<Vec<String>, SubagentError> {
    let trimmed = raw_stdout.trim();
    if trimmed.is_empty() {
        return Err(SubagentError::WorktreeSetup(
            "worktree setup hook returned empty stdout; expected JSON object".to_string(),
        ));
    }
    let parsed: serde_json::Value = serde_json::from_str(trimmed).map_err(|err| {
        SubagentError::WorktreeSetup(format!("worktree setup hook returned invalid JSON: {err}"))
    })?;
    if !parsed.is_object() {
        return Err(SubagentError::WorktreeSetup(
            "worktree setup hook stdout must be a JSON object".to_string(),
        ));
    }
    let output: WorktreeSetupHookOutput = serde_json::from_value(parsed).map_err(|err| {
        SubagentError::WorktreeSetup(format!("worktree setup hook returned invalid JSON: {err}"))
    })?;

    let Some(raw_synthetic) = output.synthetic_paths else {
        return Ok(Vec::new());
    };
    let serde_json::Value::Array(candidates) = raw_synthetic else {
        return Err(SubagentError::WorktreeSetup(
            "worktree setup hook output field 'syntheticPaths' must be an array of relative paths"
                .to_string(),
        ));
    };

    let mut unique: Vec<String> = Vec::new();
    for candidate in candidates {
        let serde_json::Value::String(candidate) = candidate else {
            return Err(SubagentError::WorktreeSetup(
                "worktree setup hook output field 'syntheticPaths' must contain only strings"
                    .to_string(),
            ));
        };
        let normalized = normalize_synthetic_path(worktree_path, &candidate)?;
        if has_tracked_entries(worktree_path, &normalized).await {
            return Err(SubagentError::WorktreeSetup(format!(
                "worktree setup hook cannot mark tracked paths as synthetic: {normalized}"
            )));
        }
        if !unique.contains(&normalized) {
            unique.push(normalized);
        }
    }
    Ok(unique)
}

/// pi `runWorktreeSetupHook` (`pi-subagents/src/runs/shared/worktree.ts:323-329` @v0.43.0): invoke
/// the hook (no args) with the worktree as cwd, the input JSON on stdin, bounded by the resolved
/// timeout, and validate its `syntheticPaths` response.
///
/// Upstream uses `spawnSync(hook.hookPath, [], { …, timeout: hook.timeoutMs })`, and Node's
/// `timeout` option KILLS the child on expiry (surfacing as `result.error.code === "ETIMEDOUT"`).
/// So must this: the `Child` binding is deliberately held OUTSIDE the `tokio::time::timeout`, and
/// the elapsed arm drives [`crate::spawn::signal::terminate_on_timeout`] (SIGTERM, then a hard
/// SIGKILL a second later). Racing a future that OWNS the child instead — which this function used
/// to do — dropped the only handle on expiry and left a hung setup hook running indefinitely.
async fn run_worktree_setup_hook(
    hook: &ResolvedWorktreeSetupHook,
    input: &WorktreeSetupHookInput<'_>,
) -> Result<Vec<String>, SubagentError> {
    let timeout = Duration::from_millis(hook.timeout_ms);
    let payload = serde_json::to_vec(input).map_err(|err| {
        SubagentError::WorktreeSetup(format!(
            "failed to serialize worktree setup hook input: {err}"
        ))
    })?;
    let worktree_path = input.worktree_path;

    let mut child = Command::new(&hook.hook_path)
        .current_dir(worktree_path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|err| {
            SubagentError::WorktreeSetup(format!("worktree setup hook failed: {err}"))
        })?;

    let call = async {
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(&payload)
                .await
                .map_err(SubagentError::Spawn)?;
            stdin.shutdown().await.map_err(SubagentError::Spawn)?;
            drop(stdin);
        }

        let mut stdout_buf = Vec::new();
        if let Some(mut stdout) = child.stdout.take() {
            stdout
                .read_to_end(&mut stdout_buf)
                .await
                .map_err(SubagentError::Spawn)?;
        }
        let mut stderr_buf = Vec::new();
        if let Some(mut stderr) = child.stderr.take() {
            let _ = stderr.read_to_end(&mut stderr_buf).await;
        }

        let status = child.wait().await.map_err(SubagentError::Spawn)?;
        if !status.success() {
            let stderr = String::from_utf8_lossy(&stderr_buf);
            let stdout = String::from_utf8_lossy(&stdout_buf);
            let details = if !stderr.trim().is_empty() {
                stderr.trim().to_string()
            } else if !stdout.trim().is_empty() {
                stdout.trim().to_string()
            } else {
                "no output".to_string()
            };
            let code = status
                .code()
                .map_or_else(|| "signal".to_string(), |c| c.to_string());
            return Err(SubagentError::WorktreeSetup(format!(
                "worktree setup hook failed with exit code {code}: {details}"
            )));
        }

        let stdout = String::from_utf8_lossy(&stdout_buf).into_owned();
        parse_and_validate_hook_output(worktree_path, &stdout).await
    };

    // Bind the race outcome in its own statement so `call` (which mutably borrows `child`) is
    // dropped before the elapsed arm needs `&mut child` again.
    let outcome = tokio::time::timeout(timeout, call).await;
    match outcome {
        Ok(result) => result,
        Err(_elapsed) => {
            // Node's `spawnSync` timeout kills; so do we. `terminate_on_timeout` returns only once
            // the OS process is confirmed reaped, so a hook that outlived its budget can never be
            // left behind holding the worktree we are about to report as failed.
            let _ = crate::spawn::signal::terminate_on_timeout(&mut child).await;
            Err(SubagentError::WorktreeSetup(format!(
                "worktree setup hook timed out after {}ms",
                hook.timeout_ms
            )))
        }
    }
}

// =================================================================================================
// Worktree creation (pi createSingleWorktree / createWorktrees)
// =================================================================================================

#[allow(clippy::too_many_arguments)]
async fn create_single_worktree(
    toplevel: &Path,
    cwd_relative: &str,
    run_id: &str,
    index: u32,
    base_commit: &str,
    setup_hook: Option<&ResolvedWorktreeSetupHook>,
    agent: Option<&str>,
    base_dir: &Path,
) -> Result<WorktreeInfo, SubagentError> {
    let branch = build_worktree_branch(run_id, index);
    let worktree_path = build_worktree_path(base_dir, run_id, index);

    let add = run_git(
        toplevel,
        &[
            "worktree",
            "add",
            &worktree_path.to_string_lossy(),
            "-b",
            &branch,
            "HEAD",
        ],
    )
    .await?;
    if add.status != Some(0) {
        let stderr = add.stderr.trim();
        let stdout = add.stdout.trim();
        let message = if !stderr.is_empty() {
            stderr.to_string()
        } else if !stdout.is_empty() {
            stdout.to_string()
        } else {
            format!("failed to create worktree {}", worktree_path.display())
        };
        return Err(SubagentError::WorktreeSetup(message));
    }

    let agent_cwd = if cwd_relative.is_empty() {
        worktree_path.clone()
    } else {
        worktree_path.join(cwd_relative)
    };

    // Everything past `worktree add` is best-effort-rolled-back on failure so a half-set-up
    // worktree is never handed back (pi createSingleWorktree's try/catch).
    let build = async {
        let node_modules_linked = link_node_modules_if_present(toplevel, &worktree_path);
        let mut synthetic_paths: Vec<String> = if node_modules_linked {
            vec!["node_modules".to_string()]
        } else {
            Vec::new()
        };

        if let Some(hook) = setup_hook {
            let hook_synthetic = run_worktree_setup_hook(
                hook,
                &WorktreeSetupHookInput {
                    version: 1,
                    repo_root: toplevel,
                    worktree_path: worktree_path.as_path(),
                    agent_cwd: agent_cwd.as_path(),
                    branch: branch.as_str(),
                    index,
                    run_id,
                    base_commit,
                    agent,
                },
            )
            .await?;
            synthetic_paths.extend(hook_synthetic);
        }

        Ok::<WorktreeInfo, SubagentError>(WorktreeInfo {
            path: worktree_path.clone(),
            agent_cwd,
            branch: branch.clone(),
            index,
            node_modules_linked,
            synthetic_paths,
        })
    }
    .await;

    match build {
        Ok(info) => Ok(info),
        Err(err) => {
            let _ = run_git(
                toplevel,
                &[
                    "worktree",
                    "remove",
                    "--force",
                    &worktree_path.to_string_lossy(),
                ],
            )
            .await;
            let _ = run_git(toplevel, &["branch", "-D", &branch]).await;
            Err(err)
        }
    }
}

// =================================================================================================
// Worktree-mutation serialization (pi `withWorktreeTransaction`, `worktree.ts:207-216` @v0.68.0)
// =================================================================================================

/// The single turn-taking lock every worktree MUTATION runs under.
///
/// pi keeps a chained `worktreeTurn: Promise<void>` plus a `setupActive` flag and asserts against
/// both (`worktree.ts:197-205`). Rust gets the same guarantee from one async mutex, and the
/// difference is not a divergence but the borrow checker paying for itself: pi needs the second
/// flag because a caller can forget to await the turn, whereas here the turn is a value a caller
/// must hold, and dropping it — including on unwind — is what releases it, exactly as pi's
/// `finally` does.
///
/// **Why it must exist.** `discardPreservedWorktrees` is only ever called inside a transaction
/// upstream (`subagent-executor.ts:6261`). Without one, `subagent({action:"worktree.discard"})`
/// can run `git worktree remove --force` against a path another task is mid-`git worktree add`
/// into, and git's own index of administrative entries is not safe against that interleaving.
///
/// **What it does NOT establish.** This is one process's mutex, so it excludes turns within this
/// process only — exactly the scope of pi's own chained promise. A second cyrup process (a second
/// editor window, a CLI invocation, a detached background runner) sharing the same repo and the
/// same manifest is not excluded by it, and this is parity with upstream rather than a cyrup
/// divergence. The safety that does hold across processes is the per-worktree gate in
/// [`cleanup_worktrees`], which probes each worktree at the moment of removal instead of trusting
/// what a ledger said earlier.
///
/// Not ported: pi's `setupPoison` (`:199`), a sticky "a previous setup left this tree in an
/// unknown state, refuse every later mutation" latch. It is set only from
/// `SetupTransaction`'s settlement-unknown path (`WorktreeSetupError`'s `snapshot.unknown`),
/// which cyrup's [`create_worktrees`] has no analogue of: its rollback is a straight-line
/// best-effort removal that never reports an indeterminate outcome. A latch nothing can ever set
/// is a gate that only looks like one.
static WORKTREE_TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Take the worktree-mutation turn, held until the returned guard is dropped.
///
/// Every caller that CREATES or REMOVES a managed worktree holds this for the whole of its
/// read-modify-write, not merely for the `git` invocation: pi's own doc on
/// `withWorktreeTransaction` is *"later async finalization owners wrap their synchronous
/// diff/cleanup as ONE turn"*, because two owners that each read a manifest, remove, and write it
/// back would otherwise lose one of the two removal ledgers.
///
/// [`create_worktrees`] takes it internally (it IS the setup pi's `setupActive` flag guards), so
/// it must never be called from inside a turn a caller already holds.
pub async fn worktree_turn() -> tokio::sync::MutexGuard<'static, ()> {
    WORKTREE_TURN.lock().await
}

/// pi `createWorktrees`: the full synchronous-before-any-spawn setup sequence. On any failure,
/// every worktree created so far is cleaned up before the error propagates.
///
/// Runs under the worktree-mutation turn ([`worktree_turn`]) for its whole duration, which is pi's
/// `setupActive` guard (`worktree.ts:197-205`): while worktrees are being allocated, no
/// `worktree.discard` may remove one out from under the `git worktree add` in flight.
///
/// # Errors
///
/// Returns [`SubagentError::WorktreeSetup`] if the tree is dirty, `cwd` is not a git repo, a
/// `git worktree add` fails, or the setup hook fails/times out/violates the synthetic-path rail.
pub async fn create_worktrees(
    cwd: &Path,
    run_id: &str,
    count: u32,
    options: Option<&CreateWorktreesOptions>,
) -> Result<WorktreeSetup, SubagentError> {
    // Held for the whole allocation — see [`worktree_turn`].
    let _turn = worktree_turn().await;
    let repo = resolve_repo_state(cwd).await?;
    let setup_hook =
        resolve_worktree_setup_hook(&repo.toplevel, options.and_then(|o| o.setup_hook.as_ref()))?;
    let base_dir =
        resolve_worktree_base_dir(options.and_then(|o| o.base_dir.as_deref()), &repo.toplevel)?;

    let mut worktrees: Vec<WorktreeInfo> = Vec::new();
    for index in 0..count {
        let agent = options
            .and_then(|o| o.agents.as_ref())
            .and_then(|agents| agents.get(index as usize))
            .map(String::as_str);
        match create_single_worktree(
            &repo.toplevel,
            &repo.cwd_relative,
            run_id,
            index,
            &repo.base_commit,
            setup_hook.as_ref(),
            agent,
            &base_dir,
        )
        .await
        {
            Ok(info) => worktrees.push(info),
            Err(err) => {
                // Allocation failed partway: unwind what was created. The report is discarded
                // here because the caller is about to return the ORIGINAL allocation error, which
                // is the diagnosis; publishing an allocation-evidence artifact is pi's separate
                // `writeWorktreeSetupHandoff` path, which has no cyrup caller.
                let _ = cleanup_worktrees(
                    &WorktreeSetup {
                        cwd: repo.toplevel.clone(),
                        worktrees,
                        base_commit: repo.base_commit.clone(),
                    },
                    &crate::handoff::WorktreeCleanupIntent::SetupRollback,
                )
                .await;
                return Err(err);
            }
        }
    }

    Ok(WorktreeSetup {
        cwd: repo.toplevel,
        worktrees,
        base_commit: repo.base_commit,
    })
}

// =================================================================================================
// Diff harvest (pi captureWorktreeDiff / diffWorktrees / formatWorktreeDiffSummary)
// =================================================================================================

fn remove_synthetic_path(worktree: &WorktreeInfo, synthetic_path: &str) {
    let resolved = lexical_normalize(&worktree.path, Path::new(synthetic_path));
    let Some(relative) = resolved.strip_prefix(&worktree.path).ok() else {
        return;
    };
    if relative.as_os_str().is_empty() {
        return;
    }
    let Ok(stat) = std::fs::symlink_metadata(&resolved) else {
        return;
    };
    if stat.file_type().is_symlink() {
        let _ = std::fs::remove_file(&resolved);
    } else if stat.is_dir() {
        let _ = std::fs::remove_dir_all(&resolved);
    } else {
        let _ = std::fs::remove_file(&resolved);
    }
}

fn remove_synthetic_paths_before_diff(worktree: &WorktreeInfo) {
    let mut seen: Vec<&str> = Vec::new();
    for synthetic_path in &worktree.synthetic_paths {
        if seen.contains(&synthetic_path.as_str()) {
            continue;
        }
        seen.push(synthetic_path.as_str());
        remove_synthetic_path(worktree, synthetic_path);
    }
}

/// pi `emptyDiff` (`worktree.ts:1056-1068`): a zero-change row, carrying `error` when it stands in
/// for a capture that FAILED rather than one that found nothing.
fn empty_diff(
    index: u32,
    agent: &str,
    branch: &str,
    patch_path: &Path,
    error: Option<String>,
) -> WorktreeDiff {
    WorktreeDiff {
        index,
        agent: agent.to_string(),
        branch: branch.to_string(),
        diff_stat: String::new(),
        files_changed: 0,
        insertions: 0,
        deletions: 0,
        patch_path: patch_path.to_path_buf(),
        error,
    }
}

fn parse_numstat(numstat: &str) -> (u64, u64, u64) {
    let mut files_changed = 0u64;
    let mut insertions = 0u64;
    let mut deletions = 0u64;
    for line in numstat.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let mut parts = line.split('\t');
        let (Some(raw_ins), Some(raw_del)) = (parts.next(), parts.next()) else {
            continue;
        };
        files_changed += 1;
        if !raw_ins.is_empty() && raw_ins.bytes().all(|b| b.is_ascii_digit()) {
            insertions += raw_ins.parse::<u64>().unwrap_or(0);
        }
        if !raw_del.is_empty() && raw_del.bytes().all(|b| b.is_ascii_digit()) {
            deletions += raw_del.parse::<u64>().unwrap_or(0);
        }
    }
    (files_changed, insertions, deletions)
}

/// pi `PATCH_VALIDATION_OPTIONS` (`worktree.ts:25`) — `git apply --check` against the index, in
/// reverse, so the check reads "does this patch describe changes the worktree already has?"
/// without applying anything.
const PATCH_VALIDATION_OPTIONS: &[&str] = &[
    "apply",
    "--check",
    "--cached",
    "--reverse",
    "--binary",
    "--whitespace=nowarn",
];

/// Validate a captured patch against the worktree index without changing either — pi
/// `validateWorktreePatch` (`worktree.ts:297-301`).
///
/// Returns `None` when the patch applies cleanly in reverse (i.e. it faithfully describes the
/// worktree's staged state), or `Some(message)` carrying pi's own `stderr -> stdout ->
/// "<command> failed"` ladder.
async fn validate_worktree_patch(worktree_path: &Path, patch_path: &Path) -> Option<String> {
    let mut args: Vec<&str> = PATCH_VALIDATION_OPTIONS.to_vec();
    let patch = patch_path.to_string_lossy();
    args.push(&patch);
    let result = run_git(worktree_path, &args).await;
    match &result {
        Ok(result) if result.status == Some(0) => None,
        _ => Some(git_failure_text(
            &result,
            &format!("git -C {} apply --check", worktree_path.display()),
        )),
    }
}

/// pi `currentWorktreePatch` (`worktree.ts:303-327`): re-capture the worktree's CURRENT diff
/// against `base_commit` **through a temporary index**, so the worktree's real `.git/index` is
/// never touched.
///
/// The re-capture uses the IDENTICAL argv to [`capture_worktree_diff`] — both go through
/// [`MACHINE_PATCH_OPTIONS`] — because this function's only callers compare the two byte for byte
/// (pi `:341`). The two sides are a matched pair: changing the flags on one without the other
/// turns the patch-preservation gate into a permanent refusal.
async fn current_worktree_patch(worktree_path: &Path, base_commit: &str) -> Result<String, String> {
    // pi mkdtemp's a directory and puts `index` inside it (`:307-311`). `GIT_INDEX_FILE` names a
    // FILE that git creates, so cyrup uses a unique temp file path directly: one fewer directory
    // to leak, and the uuid dependency is already in this crate's manifest.
    let index_file = std::env::temp_dir().join(format!(
        "cyrup-worktree-index-{}",
        uuid::Uuid::new_v4().simple()
    ));
    let env = [("GIT_INDEX_FILE", index_file.as_path())];

    let captured = async {
        let read_tree = run_git_env(worktree_path, &["read-tree", "HEAD"], &env).await;
        if !matches!(&read_tree, Ok(result) if result.status == Some(0)) {
            return Err(git_failure_text(&read_tree, "git read-tree"));
        }
        let add = run_git_env(worktree_path, &["add", "-A"], &env).await;
        if !matches!(&add, Ok(result) if result.status == Some(0)) {
            return Err(git_failure_text(&add, "git add"));
        }
        let mut diff_args: Vec<&str> = vec!["diff", "--cached"];
        diff_args.extend_from_slice(MACHINE_PATCH_OPTIONS);
        diff_args.push(base_commit);
        let diff = run_git_env(worktree_path, &diff_args, &env).await;
        match diff {
            Ok(result) if result.status == Some(0) => Ok(result.stdout),
            other => Err(git_failure_text(&other, "git diff")),
        }
    }
    .await;
    // pi's own `finally` (`:322-326`): "cleanup safety depends on preserving the worktree, not on
    // best-effort temp-index deletion", so a failure to remove the scratch index is swallowed.
    let _ = tokio::fs::remove_file(&index_file).await;
    captured
}

/// pi `validateWorktreePatchRepresentsCurrentWorktree` (`worktree.ts:329-343`).
///
/// Two independent proofs that a stored `.patch` still preserves everything the worktree holds,
/// which is what lets [`crate::spawn::cleanup_plan`] propose removing a worktree whose branch has
/// diverged from its base commit: the patch must apply cleanly in reverse against the live index,
/// AND a fresh re-capture must be byte-identical to it. Returns `None` when both hold.
///
/// Neither probe mutates the worktree: the `--check` never applies, and the re-capture stages
/// into a temporary [`GIT_INDEX_FILE`](run_git_env).
pub(crate) async fn validate_worktree_patch_represents_current_worktree(
    worktree_path: &Path,
    base_commit: &str,
    patch_path: &Path,
) -> Option<String> {
    if let Some(error) = validate_worktree_patch(worktree_path, patch_path).await {
        return Some(error);
    }
    let captured = match tokio::fs::read_to_string(patch_path).await {
        Ok(text) => text,
        Err(err) => return Some(err.to_string()),
    };
    match current_worktree_patch(worktree_path, base_commit).await {
        Err(error) if error.is_empty() => Some("failed to capture current worktree patch".into()),
        Err(error) => Some(error),
        Ok(current) if current != captured => {
            Some("captured handoff patch does not match current worktree changes".into())
        }
        Ok(_) => None,
    }
}

/// pi `captureWorktreeDiff`: strip synthetic paths, stage everything, and capture the stat/patch/
/// numstat diff against the group's base commit, writing the patch to `patch_path`.
///
/// # Errors
///
/// Returns [`SubagentError::WorktreeSetup`] on any `git` failure or if the patch file cannot be
/// written.
pub async fn capture_worktree_diff(
    setup: &WorktreeSetup,
    worktree: &WorktreeInfo,
    agent: &str,
    patch_path: &Path,
) -> Result<WorktreeDiff, SubagentError> {
    remove_synthetic_paths_before_diff(worktree);
    run_git_checked(&worktree.path, &["add", "-A"]).await?;
    let machine_diff = |extra: &'static str| {
        let mut args: Vec<&str> = vec!["diff", "--cached"];
        args.extend_from_slice(MACHINE_DIFF_OPTIONS);
        args.push(extra);
        args.push(&setup.base_commit);
        args
    };
    let diff_stat = run_git_checked(&worktree.path, &machine_diff("--stat"))
        .await?
        .trim()
        .to_string();
    let patch = {
        let mut args: Vec<&str> = vec!["diff", "--cached"];
        args.extend_from_slice(MACHINE_PATCH_OPTIONS);
        args.push(&setup.base_commit);
        run_git_checked(&worktree.path, &args).await?
    };
    let numstat = run_git_checked(&worktree.path, &machine_diff("--numstat")).await?;

    std::fs::write(patch_path, &patch).map_err(SubagentError::Spawn)?;

    if patch.trim().is_empty() {
        return Ok(empty_diff(
            worktree.index,
            agent,
            &worktree.branch,
            patch_path,
            None,
        ));
    }

    // pi `:1103-1104`. A patch that cannot be applied back is not a capture — and since the
    // harvest removes the worktree immediately afterwards, letting an unapplyable patch through
    // as a successful capture is how the work disappears. The error travels out through
    // [`diff_worktrees`] into `WorktreeDiff::error`, which the preserve gate reads.
    if let Some(validation_error) = validate_worktree_patch(&worktree.path, patch_path).await {
        return Err(SubagentError::WorktreeSetup(format!(
            "captured worktree patch is not machine-applyable: {validation_error}"
        )));
    }

    let (files_changed, insertions, deletions) = parse_numstat(&numstat);
    Ok(WorktreeDiff {
        index: worktree.index,
        agent: agent.to_string(),
        branch: worktree.branch.clone(),
        diff_stat,
        files_changed,
        insertions,
        deletions,
        patch_path: patch_path.to_path_buf(),
        error: None,
    })
}

fn write_empty_patch(patch_path: &Path) {
    let _ = std::fs::write(patch_path, "");
}

/// pi `diffWorktrees`: capture one `.patch` per worktree under `diffs_dir`, mapping any per-task
/// capture failure to an empty patch + empty diff rather than failing the whole harvest.
///
/// The failure is RETAINED, not swallowed: the substituted row carries
/// [`WorktreeDiff::error`], which is what stops [`cleanup_worktrees`]'s preserve gate from reading
/// a zero-byte placeholder patch as proof that the worktree held nothing.
pub async fn diff_worktrees(
    setup: &WorktreeSetup,
    agents: &[String],
    diffs_dir: &Path,
) -> Vec<WorktreeDiff> {
    if std::fs::create_dir_all(diffs_dir).is_err() {
        // Returning no diffs is safer than failing the whole command on artifact-dir issues.
        return Vec::new();
    }

    let mut diffs = Vec::new();
    for (index, worktree) in setup.worktrees.iter().enumerate() {
        let agent = agents
            .get(index)
            .cloned()
            .unwrap_or_else(|| format!("task-{}", index + 1));
        let patch_path = diffs_dir.join(format!(
            "task-{index}-{}.patch",
            safe_patch_agent_name(&agent)
        ));
        match capture_worktree_diff(setup, worktree, &agent, &patch_path).await {
            Ok(diff) => diffs.push(diff),
            Err(err) => {
                write_empty_patch(&patch_path);
                let idx = u32::try_from(index).unwrap_or(u32::MAX);
                diffs.push(empty_diff(
                    idx,
                    &agent,
                    &worktree.branch,
                    &patch_path,
                    Some(err.to_string()),
                ));
            }
        }
    }
    diffs
}

/// pi `cleanupWorktrees` (`runs/shared/worktree.ts`): best-effort removal of every worktree +
/// branch (reverse order), then `git worktree prune` — and, now, a REPORT of what actually
/// happened.
///
/// The report is the point. Before this returned
/// [`WorktreeCleanupReport`](crate::handoff::WorktreeCleanupReport), a removal that failed was
/// indistinguishable from one that succeeded, so nothing downstream could record that a worktree
/// is still on disk. The manifest's `group.cleanup` object IS this value
/// ([`crate::handoff::Group::cleanup`]), which is why the type is declared in
/// [`crate::handoff::model`] rather than here: it is the serialized shape, and the camelCase /
/// `deny_unknown_fields` / absent-not-null rules belong with the on-disk model.
///
/// `intent` says why removal is running, and it decides BOTH which gate a worktree that still
/// holds work must clear and what a FAILED removal records:
/// * [`WorktreeCleanupIntent::Preserve`] — the ordinary post-harvest path. A worktree holding
///   work is removed only if the harvest's [`PreserveEvidence`](crate::handoff::PreserveEvidence)
///   proves the work survives it (see [`refuse_uncaptured_preserve`]); otherwise it is kept. A
///   task that did not fully remove keeps `preserved: true`, so a later `worktree.cleanup` plan
///   still sees it.
/// * [`WorktreeCleanupIntent::Discard`] — an operator authorized removal; a worktree holding work
///   must clear pi's SECOND authority consult, and a task that did not fully remove is reported
///   with pi's `discard cleanup remains incomplete` reason (`:711`).
/// * [`WorktreeCleanupIntent::SetupRollback`] — allocation failed before any child ran, so there
///   is nothing to preserve and no probe runs at all (pi `:1171`).
pub async fn cleanup_worktrees(
    setup: &WorktreeSetup,
    intent: &crate::handoff::WorktreeCleanupIntent,
) -> crate::handoff::WorktreeCleanupReport {
    use crate::handoff::{WorktreeCleanupIntent, WorktreeCleanupTask};

    let mut tasks: Vec<WorktreeCleanupTask> = Vec::with_capacity(setup.worktrees.len());
    let mut errors: Vec<String> = Vec::new();
    // Reverse order: a worktree created later may sit inside one created earlier.
    for worktree in setup.worktrees.iter().rev() {
        if let Some(task) = refuse_unsafe_cleanup(setup, worktree, intent).await {
            tasks.push(task);
            continue;
        }
        let remove = run_git(
            &setup.cwd,
            &[
                "worktree",
                "remove",
                "--force",
                &worktree.path.to_string_lossy(),
            ],
        )
        .await;
        let worktree_removed = matches!(&remove, Ok(result) if result.status == Some(0));

        let mut task_errors: Vec<String> = Vec::new();
        if !worktree_removed {
            task_errors.push(git_failure_text(&remove, "git worktree remove"));
        }
        // pi `:1253-1263`: the branch is deleted ONLY if the worktree really went away. A removal
        // that failed leaves the work on disk, and its branch may be the only ref reaching the
        // commits the child made — deleting it anyway turns a recoverable failure into a second
        // data-loss path. (git declines to delete a branch a worktree is checked out on, but a
        // child that detached HEAD inside its worktree removes that protection, which is exactly
        // the case where the commits matter.)
        let mut branch_removed = false;
        if worktree_removed {
            let branch = run_git(&setup.cwd, &["branch", "-D", &worktree.branch]).await;
            branch_removed = matches!(&branch, Ok(result) if result.status == Some(0));
            if !branch_removed {
                task_errors.push(git_failure_text(&branch, "git branch -D"));
            }
        }
        let fully_removed = worktree_removed && branch_removed;
        let reason = match intent {
            _ if fully_removed => None,
            WorktreeCleanupIntent::Discard { .. } => {
                Some("discard cleanup remains incomplete".to_string())
            }
            WorktreeCleanupIntent::Preserve(_) => {
                Some("worktree removal did not complete".to_string())
            }
            WorktreeCleanupIntent::SetupRollback => {
                Some("setup rollback did not complete".to_string())
            }
        };
        tasks.push(WorktreeCleanupTask {
            index: worktree.index,
            path: worktree.path.clone(),
            branch: worktree.branch.clone(),
            // pi copies the allocating provider onto every cleanup row
            // (`worktree.ts:1272`; the native allocator stamps `provider: "native"` at `:929`).
            // cyrup has exactly one allocator — `git worktree add`, in
            // [`create_single_worktree`] — so the value is a constant here rather than a field on
            // [`WorktreeInfo`]: a per-worktree field that can only ever hold one value is a place
            // for the two to disagree.
            provider: Some(crate::workflows::ManagedWorktreeProvider::Native),
            // cyrup's branch/path names come straight from [`build_worktree_path`] and the
            // `subagents/<run>-<index>` branch shape, with no requested-branch, no label and no
            // collision handling, so there is no naming EVIDENCE to retain. Written absent rather
            // than fabricated; a manifest written by pi (or by a future cyrup that gains a
            // naming provider) round-trips it.
            naming: None,
            worktree_removed,
            branch_removed,
            // A worktree still on disk is PRESERVED, whatever the intent was: the next reader
            // must be able to tell "still there" from "gone".
            preserved: (!fully_removed).then_some(true),
            reason,
            errors: (!task_errors.is_empty()).then_some(task_errors),
        });
    }
    // Restore task order: the removal loop runs in reverse, but the ledger is read by index.
    tasks.sort_by_key(|task| task.index);

    let prune = run_git(&setup.cwd, &["worktree", "prune"]).await;
    let pruned = matches!(&prune, Ok(result) if result.status == Some(0));
    if !pruned {
        errors.push(git_failure_text(&prune, "git worktree prune"));
    }

    let all_removed = tasks
        .iter()
        .all(|task| task.worktree_removed && task.branch_removed);
    crate::handoff::WorktreeCleanupReport {
        // pi `:714-717`: `complete` requires BOTH every task doubly-removed AND a successful
        // prune. A stale administrative entry left behind by a failed prune is not "complete".
        state: if all_removed && pruned {
            crate::handoff::CleanupState::Complete
        } else {
            crate::handoff::CleanupState::Partial
        },
        tasks,
        pruned,
        errors: (!errors.is_empty()).then_some(errors),
    }
}

/// What the pre-removal probe found in a worktree — pi's `status` / `baseDiff` pair
/// (`worktree.ts:1177-1197` @v0.68.0), as a value instead of two loose exit codes.
///
/// The three-way shape is the point: `git diff --quiet` exits 1 for *differs* and 0 for
/// *identical*, so anything else is the probe failing to answer, and an unanswered probe is never
/// collapsed into [`WorktreeWorkProbe::Clean`].
enum WorktreeWorkProbe {
    /// Neither `git status --porcelain` nor the base-commit diff found anything.
    Clean,
    /// Uncommitted changes, untracked files, or divergence from the group's base commit.
    ///
    /// Both probes are needed and neither subsumes the other: `git status --porcelain` sees
    /// untracked files a base diff does not, and the base diff sees committed divergence a clean
    /// status does not.
    HasWork,
    /// The probe did not answer, carrying pi's reported text (`:1180-1192`).
    Unanswered(String),
}

/// Run both probes against `worktree`.
async fn probe_worktree_work(setup: &WorktreeSetup, worktree: &WorktreeInfo) -> WorktreeWorkProbe {
    let status = run_git(&worktree.path, &["status", "--porcelain"]).await;
    let mut base_diff_args: Vec<&str> = vec!["diff", "--quiet"];
    base_diff_args.extend_from_slice(MACHINE_DIFF_OPTIONS);
    base_diff_args.push(&setup.base_commit);
    base_diff_args.push("--");
    let base_diff = run_git(&worktree.path, &base_diff_args).await;

    let status_ok = matches!(&status, Ok(result) if result.status == Some(0));
    let diff_answered =
        matches!(&base_diff, Ok(result) if result.status == Some(0) || result.status == Some(1));
    if !status_ok || !diff_answered {
        // pi `:1180-1192`, including which of the two probes' text is reported.
        return WorktreeWorkProbe::Unanswered(if status_ok {
            git_failure_text(&base_diff, "git diff check")
        } else {
            git_failure_text(&status, "git status")
        });
    }

    let dirty = status
        .as_ref()
        .is_ok_and(|result| !result.stdout.trim().is_empty());
    let diverged = matches!(&base_diff, Ok(result) if result.status == Some(1));
    if dirty || diverged {
        WorktreeWorkProbe::HasWork
    } else {
        WorktreeWorkProbe::Clean
    }
}

/// pi `cleanupSingleWorktree`'s pre-removal gates (`runs/shared/worktree.ts:1171-1252` @v0.68.0),
/// returning `Some(task)` when the worktree must be PRESERVED instead of removed.
///
/// Three ways to land there, all of them pi's:
/// * the probe itself did not answer (`:1180-1192`) — "is there work here" is unknown, and
///   unknown is never treated as "no";
/// * there IS work, the intent is `preserve`, and the harvest's evidence does not prove the work
///   survives removal (`:1197-1231`);
/// * there IS work, the intent is `discard`, and the authorization does not clear it
///   (`:1232-1252`).
///
/// [`WorktreeCleanupIntent::SetupRollback`] skips all three (pi `:1171`): allocation failed before
/// any child ran, so the only thing in those worktrees is the allocation this call is unwinding.
async fn refuse_unsafe_cleanup(
    setup: &WorktreeSetup,
    worktree: &WorktreeInfo,
    intent: &crate::handoff::WorktreeCleanupIntent,
) -> Option<crate::handoff::WorktreeCleanupTask> {
    use crate::handoff::WorktreeCleanupIntent;

    let preserved = |reason: &str, error: String| crate::handoff::WorktreeCleanupTask {
        index: worktree.index,
        path: worktree.path.clone(),
        branch: worktree.branch.clone(),
        provider: Some(crate::workflows::ManagedWorktreeProvider::Native),
        naming: None,
        worktree_removed: false,
        branch_removed: false,
        preserved: Some(true),
        reason: Some(reason.to_string()),
        errors: Some(vec![error]),
    };

    if matches!(intent, WorktreeCleanupIntent::SetupRollback) {
        return None;
    }

    // pi `:1172-1176`. Scaffolding this module created (the `node_modules` symlink, hook-declared
    // synthetic paths) is not the child's work, and leaving it in place would make every probe
    // below read "dirty" and every re-capture differ from the stored patch — a refusal caused by
    // cyrup's own setup rather than by anything at risk. [`remove_synthetic_path`] swallows its
    // own I/O errors, so unlike pi there is no error to collect here; a path that would not go
    // away simply keeps the worktree, which is the safe direction.
    remove_synthetic_paths_before_diff(worktree);

    match probe_worktree_work(setup, worktree).await {
        WorktreeWorkProbe::Unanswered(detail) => {
            return Some(preserved(
                "cleanup safety check failed",
                format!("cleanup refused: {detail}"),
            ));
        }
        WorktreeWorkProbe::Clean => return None,
        WorktreeWorkProbe::HasWork => {}
    }

    let reason = match intent {
        // pi `:1197-1231`. The harvest path's gate: this worktree holds work, and it is about to
        // be `--force`-removed on the ORDINARY success path, with nobody asked. The only thing
        // that makes that safe is a captured patch that demonstrably still represents it.
        WorktreeCleanupIntent::Preserve(evidence) => {
            refuse_uncaptured_preserve(setup, worktree, evidence).await?
        }
        // pi's SECOND authority consult (`:1232-1252`), the one
        // `subagent({action:"worktree.discard"})` makes reachable by a model.
        //
        // The dispatch-level consult answers "may this session discard at all"; THIS one answers
        // "may it discard a worktree that still holds uncommitted work". They are not the same
        // question, and collapsing them would let a policy of `discardWorktree: "auto"` delete a
        // developer's unpushed changes with nobody asked. Under `confirm`, only an authorization
        // that actually came from the user clears it (pi `:1234`).
        WorktreeCleanupIntent::Discard { authorization } => {
            refuse_unauthorized_discard(*authorization)?
        }
        // Already returned above, before the probe ran. Spelled out rather than folded into a
        // `_` arm: a wildcard here would silently route a FUTURE intent past both gates, which is
        // the shape of the defect this function exists to close.
        WorktreeCleanupIntent::SetupRollback => return None,
    };

    Some(preserved(
        &reason,
        format!(
            "cleanup refused: {reason}; preserved {}",
            worktree.path.display()
        ),
    ))
}

/// pi's captured-patch gate (`worktree.ts:1197-1231` @v0.68.0): may a worktree that still holds
/// work be removed on the harvest path?
///
/// Returns `None` to allow removal, or `Some(reason)` to refuse. It allows removal only when ALL
/// of the following hold, and every one of them closes a distinct way the work can vanish:
/// * the harvest produced a capture row for this worktree — otherwise nothing was even attempted;
/// * that row carries no [`WorktreeDiff::error`] — a failed capture writes a zero-byte placeholder
///   patch, and reading that as "nothing changed" is precisely how a failed capture becomes a
///   silent deletion;
/// * the `.patch` file exists and is non-empty;
/// * the MANIFEST already records that patch
///   ([`handoff_records_patch`](crate::handoff::handoff_records_patch)) — a patch no durable
///   record names cannot be found by any recovery path;
/// * a fresh re-capture is byte-identical to it and it applies cleanly in reverse
///   ([`validate_worktree_patch_represents_current_worktree`]) — the patch must describe the
///   worktree as it is NOW, not as it was when the child finished.
async fn refuse_uncaptured_preserve(
    setup: &WorktreeSetup,
    worktree: &WorktreeInfo,
    evidence: &crate::handoff::PreserveEvidence,
) -> Option<String> {
    let captured = evidence
        .captured_diffs
        .iter()
        .find(|diff| diff.index == worktree.index);

    let mut patch_validation_error: Option<String> = None;
    let mut patch_captured = false;
    if let Some(captured) = captured
        && captured.error.is_none()
        && tokio::fs::try_exists(&captured.patch_path)
            .await
            .unwrap_or(false)
        && crate::handoff::handoff_records_patch(
            evidence.handoff_manifest_path.as_deref(),
            &captured.patch_path,
        )
        .await
    {
        match tokio::fs::metadata(&captured.patch_path).await {
            // pi `:1206`: a zero-byte patch is not a capture, and it is not a validation failure
            // either — it leaves `patch_captured` false with no error, so the refusal names the
            // missing representation rather than inventing a validation message.
            Ok(metadata) if metadata.len() == 0 => {}
            Ok(_) => {
                patch_validation_error = validate_worktree_patch_represents_current_worktree(
                    &worktree.path,
                    &setup.base_commit,
                    &captured.patch_path,
                )
                .await;
                patch_captured = patch_validation_error.is_none();
            }
            Err(err) => patch_validation_error = Some(err.to_string()),
        }
    }

    if patch_captured {
        return None;
    }
    // pi `:1216-1218`, verbatim.
    Some(match patch_validation_error {
        Some(error) => format!("captured handoff patch failed validation: {error}"),
        None => "worktree contains changes that are not represented by a captured handoff patch"
            .to_string(),
    })
}

/// pi's second authority consult (`worktree.ts:1232-1252` @v0.68.0): may a worktree that still
/// holds work be `--force`-removed under this authorization?
///
/// Returns `None` to allow removal, or `Some(reason)` to refuse.
fn refuse_unauthorized_discard(
    authorization: crate::handoff::DiscardAuthorization,
) -> Option<String> {
    use crate::registration::authority as auth;

    // pi `:1233-1234`. `resolve_authority_decision` falls back to `default_decision`, which is
    // `Confirm` for `discardWorktree` — so an ABSENT policy is never an implicit yes.
    let decision = auth::resolve_authority_decision(
        auth::AuthorityAction::DiscardWorktree,
        authorization.policy.as_ref(),
    );
    let authorized = match decision {
        auth::AuthorityDecision::Auto => true,
        auth::AuthorityDecision::Confirm => {
            authorization.kind == crate::handoff::DiscardAuthorizationKind::Confirmed
        }
        auth::AuthorityDecision::Forbid => false,
    };
    if authorized {
        return None;
    }
    // pi `:1236-1238`, verbatim.
    Some(
        if decision == auth::AuthorityDecision::Forbid {
            "authority policy forbids worktree discard"
        } else {
            "worktree discard requires explicit user confirmation"
        }
        .to_string(),
    )
}

/// pi `gitFailure`'s `stderr -> stdout -> "<command> failed"` ladder, reused for the per-task
/// removal errors the report carries.
fn git_failure_text(result: &Result<GitResult, SubagentError>, command: &str) -> String {
    match result {
        Ok(result) => {
            let stderr = result.stderr.trim();
            let stdout = result.stdout.trim();
            if !stderr.is_empty() {
                stderr.to_string()
            } else if !stdout.is_empty() {
                stdout.to_string()
            } else {
                format!("{command} failed")
            }
        }
        Err(err) => format!("{command} failed: {err}"),
    }
}

fn has_worktree_changes(diff: &WorktreeDiff) -> bool {
    diff.files_changed > 0
        || diff.insertions > 0
        || diff.deletions > 0
        || !diff.diff_stat.trim().is_empty()
}

/// pi `formatWorktreeDiffSummary`: a human-readable summary of the changed worktrees, or the empty
/// string when nothing changed.
#[must_use]
pub fn format_worktree_diff_summary(diffs: &[WorktreeDiff]) -> String {
    let changed: Vec<&WorktreeDiff> = diffs.iter().filter(|d| has_worktree_changes(d)).collect();
    let Some(first) = changed.first() else {
        return String::new();
    };

    let mut lines: Vec<String> = vec!["=== Worktree Changes ===".to_string(), String::new()];
    for diff in &changed {
        lines.push(format!(
            "--- Task {} ({}): {} files changed, +{} -{} ---",
            diff.index + 1,
            diff.agent,
            diff.files_changed,
            diff.insertions,
            diff.deletions
        ));
        if !diff.diff_stat.trim().is_empty() {
            lines.push(diff.diff_stat.clone());
        }
        lines.push(String::new());
    }

    let patches_dir = first
        .patch_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    lines.push(format!("Full patches: {}", patches_dir.display()));
    lines.join("\n").trim_end().to_string()
}

// =================================================================================================
// Group-level wrappers over the pi-faithful primitives
// =================================================================================================

/// The configured external hook command [`WorktreeGroupConfig`] accepts — an alias of the
/// canonical [`crate::registration::HookSpec`] (arch-SA §2.2 designates `registration/mod.rs` as
/// its owner).
///
/// The pi-faithful hook contract is [`WorktreeSetupHookConfig`]; this shape's `command` maps onto
/// `hook_path` and its `args` are currently ignored by the per-worktree invocation.
pub type HookSpec = crate::registration::HookSpec;

/// Legacy per-group config accepted by [`setup_worktree_group`].
#[derive(Debug, Clone)]
pub struct WorktreeGroupConfig<'a> {
    /// A stable id for this fan-out group (used as the pi `runId`).
    pub group_id: &'a str,
    /// Directory new worktrees are created under.
    ///
    /// `None` means "unconfigured", and is handed straight to
    /// [`resolve_worktree_base_dir`], which applies the SAME
    /// `$CYRUP_SUBAGENTS_WORKTREE_DIR` -> [`std::env::temp_dir`] ladder [`create_worktrees`] has
    /// always applied to its own `base_dir` option. It used to be a required `&Path`, and the one
    /// production caller ([`crate::spawn::chain_graph`]'s `assign_worktree_cwds`) turned a `None`
    /// config into a hard error — which made `worktree: true` fail in a DEFAULT install, because
    /// nothing in `registration/` ever computes a fallback. Passing the `Option` through is what
    /// makes the flag the tool schema already advertises actually work out of the box.
    pub worktree_base_dir: Option<&'a Path>,
    /// The optional setup hook.
    pub setup_hook: Option<&'a HookSpec>,
    /// Bound on the setup hook's runtime, in milliseconds.
    pub setup_hook_timeout_ms: Option<u64>,
}

/// Legacy per-task worktree assignment. `path` is the child's actual `cwd` (pi `agentCwd`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeAssignment {
    /// The directory the child MUST run in (the pi `agentCwd`).
    pub path: PathBuf,
    /// The branch created for this worktree.
    pub branch: String,
    /// The group's common base commit.
    pub base_commit: String,
    /// The task's 0-based index.
    pub index: u32,
    /// Worktree-relative synthetic paths declared for this worktree.
    pub synthetic_paths: Vec<PathBuf>,
}

/// Legacy plan returned by [`setup_worktree_group`], now beside the full allocation it was
/// derived from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeGroupPlan {
    /// One assignment per task, in task order.
    pub assignments: Vec<WorktreeAssignment>,
    /// The group's common base commit.
    pub base_commit: String,
    /// The full [`WorktreeSetup`] `create_worktrees` produced.
    ///
    /// This used to be DISCARDED — the plan kept only the per-task cwd and the base commit. The
    /// handoff manifest needs the rest of it: `setup.cwd` is the manifest's `repoRoot`, and
    /// `setup.worktrees[].{index,path,branch}` are its cleanup tasks
    /// (pi `parallel-handoff.ts:530-531,:574-584`). Cleanup needs it too — `cleanup_worktrees`
    /// takes a whole `&WorktreeSetup`. Rebuilding it from the assignments is not possible:
    /// [`WorktreeAssignment::path`] is the child's `agent_cwd`, which is the worktree root ONLY
    /// when the shared cwd is the repo toplevel.
    pub setup: WorktreeSetup,
}

impl WorktreeGroupPlan {
    /// Number of tasks (and worktrees) in this plan.
    #[must_use]
    pub fn task_count(&self) -> usize {
        self.assignments.len()
    }
}

/// Legacy entry point retained for `spawn::chain_graph::assign_worktree_cwds`. Delegates to the
/// pi-faithful [`create_worktrees`], returning each worktree's `agent_cwd` as the assignment path.
///
/// Unlike the old strict all-overrides-rejected behavior, this now allows a task `cwd` equal to
/// the shared cwd (pi `findWorktreeTaskCwdConflict`), rejecting only genuinely divergent overrides.
///
/// # Errors
///
/// Propagates [`create_worktrees`]' errors, or [`SubagentError::WorktreeSetup`] if a task declares
/// a divergent `cwd` override.
pub async fn setup_worktree_group(
    repo_cwd: &Path,
    task_cwd_overrides: &[Option<&Path>],
    config: &WorktreeGroupConfig<'_>,
) -> Result<WorktreeGroupPlan, SubagentError> {
    let owned_cwds: Vec<Option<String>> = task_cwd_overrides
        .iter()
        .map(|c| c.map(|p| p.to_string_lossy().into_owned()))
        .collect();
    let tasks: Vec<(&str, Option<&str>)> =
        owned_cwds.iter().map(|c| ("task", c.as_deref())).collect();
    if let Some(conflict) = find_worktree_task_cwd_conflict(&tasks, repo_cwd) {
        return Err(SubagentError::WorktreeSetup(
            format_worktree_task_cwd_conflict(&conflict, repo_cwd),
        ));
    }

    let setup_hook = config.setup_hook.map(|hook| WorktreeSetupHookConfig {
        hook_path: hook.command.to_string_lossy().into_owned(),
        timeout_ms: config.setup_hook_timeout_ms,
    });
    let options = CreateWorktreesOptions {
        agents: None,
        setup_hook,
        base_dir: config
            .worktree_base_dir
            .map(|dir| dir.to_string_lossy().into_owned()),
    };

    let count = u32::try_from(task_cwd_overrides.len()).unwrap_or(u32::MAX);
    let setup = create_worktrees(repo_cwd, config.group_id, count, Some(&options)).await?;

    let assignments = setup
        .worktrees
        .iter()
        .map(|w| WorktreeAssignment {
            path: w.agent_cwd.clone(),
            branch: w.branch.clone(),
            base_commit: setup.base_commit.clone(),
            index: w.index,
            synthetic_paths: w.synthetic_paths.iter().map(PathBuf::from).collect(),
        })
        .collect();

    Ok(WorktreeGroupPlan {
        assignments,
        base_commit: setup.base_commit.clone(),
        setup,
    })
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]

    use super::*;
    use std::process::Command as StdCommand;

    /// The authorization these rows discard under: a policy that already answered `auto`, so the
    /// dirty-worktree gate (`refuse_unauthorized_dirty_discard`) clears and the rows keep
    /// exercising REMOVAL. The gate's own refusal path is covered in
    /// `extension/tool/lane_actions.rs`, through the real verb.
    fn test_discard_authorization() -> crate::handoff::DiscardAuthorization {
        crate::handoff::DiscardAuthorization {
            kind: crate::handoff::DiscardAuthorizationKind::Policy,
            policy: Some(crate::registration::authority::AuthorityPolicyConfig {
                discard_worktree: Some(crate::registration::authority::AuthorityDecision::Auto),
                ..Default::default()
            }),
        }
    }

    /// A real, throwaway git repo with one committed file, a `.gitignore` ignoring `node_modules/`,
    /// and a tracked `tracked.txt` — mirrors pi's worktree.test.ts `createRepo`.
    fn make_real_git_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("real tempdir");
        let run = |args: &[&str]| {
            let status = StdCommand::new("git")
                .current_dir(dir.path())
                .args(args)
                .status()
                .expect("git spawns");
            assert!(status.success(), "git {args:?} must succeed in the fixture");
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "Worktree Tests"]);
        std::fs::write(dir.path().join(".gitignore"), "node_modules/\n").expect("gitignore");
        std::fs::write(dir.path().join("tracked.txt"), "initial\n").expect("tracked");
        run(&["add", "-A"]);
        run(&["commit", "-q", "-m", "initial commit"]);
        dir
    }

    fn git(cwd: &Path, args: &[&str]) -> String {
        let out = StdCommand::new("git")
            .current_dir(cwd)
            .args(args)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// SUBA-069 — the env var a hook script honours to exit before it does any work, so
    /// [`warm_hook_exec`] can pay macOS's one-off first-`exec` verification cost for this exact
    /// script content WITHOUT running the body (which would block on `cat`, create files, or
    /// `sleep 30`).
    ///
    /// Same mechanism SUBA-068's `$WARMUP` guard uses in
    /// `a_timed_out_setup_hook_is_killed_not_abandoned`; hoisted here so the whole hook family gets
    /// it instead of the one test that was measured.
    #[cfg(unix)]
    const HOOK_WARMUP_ENV: &str = "CYRUP_HOOK_WARMUP";

    /// SUBA-069 — pay macOS's first-`exec` verification for `hook` before any timed run touches it.
    ///
    /// macOS charges a one-off verification cost on the first `exec` of a freshly written
    /// executable whose exact content it has not seen; measured in
    /// `a_timed_out_setup_hook_is_killed_not_abandoned` at 197-242ms for unique content versus
    /// ~0.2ms once seen. Every hook fixture here writes unique content (the body, and for the
    /// repo-relative fixture a randomized tempdir path, differ per test), so that cost is paid
    /// inside the hook's own timeout budget on EVERY run — which is the dominant term in the
    /// load-induced `worktree setup hook timed out after …ms` failures SUBA-069 measured.
    ///
    /// Running it once here, outside any budget, removes that term instead of guessing a number.
    /// Best-effort: a failure to warm only restores the old timing, it never fails the test.
    #[cfg(unix)]
    fn warm_hook_exec(hook: &Path) {
        let _ = std::process::Command::new(hook)
            .env(HOOK_WARMUP_ENV, "1")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }

    #[cfg(not(unix))]
    fn warm_hook_exec(_hook: &Path) {}

    fn write_hook_script(body: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("hook dir");
        let hook = dir.path().join("hook.sh");
        // SUBA-069: the warm-up guard is the FIRST line so `warm_hook_exec` returns before the body
        // consumes stdin or writes anything. It is inert for the real run, which never sets the var.
        #[cfg(unix)]
        let source = format!("#!/bin/sh\n[ -n \"${HOOK_WARMUP_ENV}\" ] && exit 0\n{body}\n");
        #[cfg(not(unix))]
        let source = format!("#!/bin/sh\n{body}\n");
        std::fs::write(&hook, source).expect("write hook");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        warm_hook_exec(&hook);
        (dir, hook)
    }

    /// SUBA-069 regression: the warm-up guard must short-circuit BEFORE the body runs, or
    /// [`warm_hook_exec`] would execute the fixture's side effects (create `.env.local`, consume
    /// stdin, `sleep 30`) once per test and the cure would be worse than the flake.
    ///
    /// RED before the fix: `write_hook_script` emitted no guard line at all, so this run would
    /// create the marker.
    #[cfg(unix)]
    #[test]
    fn a_warmed_hook_script_exits_before_its_body_runs() {
        let (dir, hook) = write_hook_script("touch body-ran");
        // `write_hook_script` already warmed it once; do it again explicitly so the assertion is
        // about the guard rather than about how many times the helper happened to run.
        warm_hook_exec(&hook);
        assert!(
            !dir.path().join("body-ran").exists(),
            "the warm-up exec must not run the hook body"
        );

        // …and the same script without the guard variable DOES run its body, so the guard is the
        // only thing suppressing it.
        let status = std::process::Command::new(&hook)
            .current_dir(dir.path())
            .status()
            .expect("run hook");
        assert!(status.success());
        assert!(
            dir.path().join("body-ran").exists(),
            "an unwarmed run must still execute the body"
        );
    }

    // ---- structure / cwd mapping / base-dir ----

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn create_worktrees_returns_expected_structure() {
        let repo = make_real_git_repo();
        let setup = create_worktrees(repo.path(), "structure", 2, None)
            .await
            .expect("create");
        assert_eq!(setup.worktrees.len(), 2);
        assert_eq!(
            setup.cwd,
            PathBuf::from(git(repo.path(), &["rev-parse", "--show-toplevel"]))
        );
        for (i, wt) in setup.worktrees.iter().enumerate() {
            assert_eq!(wt.branch, format!("cyrup-parallel-structure-{i}"));
            assert_eq!(wt.index, u32::try_from(i).unwrap());
            assert_eq!(wt.agent_cwd, wt.path);
            assert!(!wt.node_modules_linked);
            assert!(wt.synthetic_paths.is_empty());
            assert!(wt.path.is_dir());
        }
        cleanup_worktrees(
            &setup,
            &crate::handoff::WorktreeCleanupIntent::Discard {
                authorization: test_discard_authorization(),
            },
        )
        .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn create_worktrees_maps_subdirectory_cwd_to_agent_cwd() {
        let repo = make_real_git_repo();
        let nested = repo.path().join("packages").join("app");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("index.ts"), "export const v = 1;\n").unwrap();
        git(repo.path(), &["add", "-A"]);
        git(repo.path(), &["commit", "-q", "-m", "add nested"]);

        let setup = create_worktrees(&nested, "subdir", 1, None)
            .await
            .expect("create");
        assert_eq!(
            setup.worktrees[0].agent_cwd,
            setup.worktrees[0].path.join("packages").join("app")
        );
        cleanup_worktrees(
            &setup,
            &crate::handoff::WorktreeCleanupIntent::Discard {
                authorization: test_discard_authorization(),
            },
        )
        .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn creates_worktrees_under_a_configured_base_directory() {
        let repo = make_real_git_repo();
        let base_parent = tempfile::tempdir().unwrap();
        let base_dir = base_parent.path().join("nested");
        let options = CreateWorktreesOptions {
            base_dir: Some(base_dir.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let setup = create_worktrees(repo.path(), "base-dir", 1, Some(&options))
            .await
            .expect("create");
        assert_eq!(
            setup.worktrees[0].path,
            base_dir.join("cyrup-worktree-base-dir-0")
        );
        assert!(base_dir.exists());
        cleanup_worktrees(
            &setup,
            &crate::handoff::WorktreeCleanupIntent::Discard {
                authorization: test_discard_authorization(),
            },
        )
        .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn create_worktrees_rejects_dirty_repositories() {
        let repo = make_real_git_repo();
        std::fs::write(repo.path().join("tracked.txt"), "dirty\n").unwrap();
        let err = create_worktrees(repo.path(), "dirty", 1, None)
            .await
            .expect_err("dirty rejects");
        let SubagentError::WorktreeSetup(msg) = err else {
            panic!("wrong variant")
        };
        assert!(msg.contains("clean git working tree"), "{msg}");
        // No worktree was created.
        let list = git(repo.path(), &["worktree", "list", "--porcelain"]);
        assert_eq!(list.matches("worktree ").count(), 1);
    }

    // ---- cwd-conflict (allow-equal) ----

    #[test]
    fn conflict_allows_omitted_or_matching_task_cwd() {
        let shared = Path::new("/tmp/repo");
        assert!(
            find_worktree_task_cwd_conflict(
                &[("worker-a", None), ("worker-b", Some("/tmp/repo"))],
                shared
            )
            .is_none()
        );
    }

    #[test]
    fn conflict_allows_relative_dot_task_cwd() {
        let shared = Path::new("/tmp/repo");
        assert!(find_worktree_task_cwd_conflict(&[("worker-a", Some("."))], shared).is_none());
    }

    #[test]
    fn conflict_returns_first_divergent_task_cwd() {
        let shared = Path::new("/tmp/repo");
        let conflict = find_worktree_task_cwd_conflict(
            &[
                ("worker-a", Some("/tmp/repo")),
                ("worker-b", Some("/tmp/repo/packages/app")),
            ],
            shared,
        )
        .expect("conflict");
        assert_eq!(conflict.index, 1);
        assert_eq!(conflict.agent, "worker-b");
        assert_eq!(conflict.cwd, "/tmp/repo/packages/app");
    }

    // ---- MANDATED: a successful worktree group captures per-task diffs and cleans up ----

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn successful_group_captures_per_task_diffs_and_cleans_up() {
        let repo = make_real_git_repo();
        // A node_modules dir that must NOT appear in any diff (symlinked + synthetic).
        let node_modules = repo.path().join("node_modules");
        std::fs::create_dir_all(&node_modules).unwrap();
        std::fs::write(node_modules.join("fixture.txt"), "fixture\n").unwrap();

        let base = tempfile::tempdir().unwrap();
        let options = CreateWorktreesOptions {
            base_dir: Some(base.path().to_string_lossy().into_owned()),
            ..Default::default()
        };
        let setup = create_worktrees(repo.path(), "diff", 2, Some(&options))
            .await
            .expect("create");
        assert_eq!(setup.worktrees.len(), 2);

        // Each worktree does distinct work: committed, modified, and new files.
        for (i, wt) in setup.worktrees.iter().enumerate() {
            std::fs::write(
                wt.path.join("committed.ts"),
                format!("export const c{i} = true;\n"),
            )
            .unwrap();
            git(&wt.path, &["add", "committed.ts"]);
            git(&wt.path, &["commit", "-q", "-m", "committed change"]);
            std::fs::write(wt.path.join("tracked.txt"), format!("modified-{i}\n")).unwrap();
            std::fs::write(wt.path.join("new-file.ts"), "export const added = true;\n").unwrap();
        }

        let diffs_dir = repo.path().join("artifacts").join("worktree-diffs");
        let agents = vec!["agent-a".to_string(), "agent-b".to_string()];
        let diffs = diff_worktrees(&setup, &agents, &diffs_dir).await;

        assert_eq!(diffs.len(), 2);
        for (i, diff) in diffs.iter().enumerate() {
            assert_eq!(diff.agent, agents[i]);
            assert_eq!(
                diff.files_changed, 3,
                "3 files per worktree, got {}",
                diff.files_changed
            );
            assert!(diff.insertions > 0);
            assert!(diff.patch_path.exists(), "per-task patch file must exist");
            let patch = std::fs::read_to_string(&diff.patch_path).unwrap();
            assert!(patch.contains("committed.ts"));
            assert!(patch.contains("tracked.txt"));
            assert!(patch.contains("new-file.ts"));
            // node_modules symlink was stripped before diffing — never leaks in.
            assert!(
                !patch.contains("diff --git a/node_modules b/node_modules"),
                "{patch}"
            );
        }

        let summary = format_worktree_diff_summary(&diffs);
        assert!(summary.contains("=== Worktree Changes ==="));
        assert!(summary.contains("Full patches:"));

        // Success-path cleanup: worktrees and branches are gone (C18).
        let paths: Vec<PathBuf> = setup.worktrees.iter().map(|w| w.path.clone()).collect();
        let branches: Vec<String> = setup.worktrees.iter().map(|w| w.branch.clone()).collect();
        cleanup_worktrees(
            &setup,
            &crate::handoff::WorktreeCleanupIntent::Discard {
                authorization: test_discard_authorization(),
            },
        )
        .await;
        for path in &paths {
            assert!(
                !path.exists(),
                "worktree {} must be removed",
                path.display()
            );
        }
        for branch in &branches {
            let listed = git(repo.path(), &["branch", "--list", branch]);
            assert!(listed.trim().is_empty(), "branch {branch} must be deleted");
        }
        let list = git(repo.path(), &["worktree", "list", "--porcelain"]);
        assert_eq!(list.matches("worktree ").count(), 1, "only primary remains");
    }

    /// LANES_2 / QA-MAJOR — a child's BINARY output must survive the harvest.
    ///
    /// THE USER ACTION: a fan-out child generates a PNG snapshot, a `.wasm`, a fixture blob. Its
    /// worktree is force-removed the moment the harvest reports success, so the captured `.patch`
    /// is the only copy. Without pi's `--binary` (`MACHINE_PATCH_OPTIONS`, `worktree.ts:24`) git
    /// writes `Binary files … differ` — a sentence about the change rather than the change — and
    /// the bytes are gone. The `git apply` below is the assertion that matters: a patch that
    /// cannot be applied is not a backup.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_captured_patch_carries_binary_content_not_a_note_about_it() {
        let repo = make_real_git_repo();
        let base = tempfile::tempdir().unwrap();
        let options = CreateWorktreesOptions {
            base_dir: Some(base.path().to_string_lossy().into_owned()),
            ..Default::default()
        };
        let setup = create_worktrees(repo.path(), "binarywork", 1, Some(&options))
            .await
            .expect("create");
        let worktree = setup.worktrees[0].clone();

        // Bytes git will classify as binary: a NUL inside the first 8000.
        let blob: Vec<u8> = (0u8..=255).cycle().take(4096).collect();
        std::fs::write(worktree.path.join("asset.bin"), &blob).unwrap();

        let diffs_dir = repo.path().join("diffs");
        let diffs = diff_worktrees(&setup, &["agent-a".to_string()], &diffs_dir).await;
        assert!(diffs[0].error.is_none(), "{:?}", diffs[0].error);

        let patch = std::fs::read(&diffs[0].patch_path).unwrap();
        let patch_text = String::from_utf8_lossy(&patch);
        assert!(
            patch_text.contains("GIT binary patch"),
            "the patch must carry the bytes, not describe them: {patch_text}"
        );

        // And it really reconstructs the file: apply it to a clean checkout of the base commit.
        let replay = tempfile::tempdir().unwrap();
        let replay_path = replay.path().join("replay");
        git(
            repo.path(),
            &[
                "worktree",
                "add",
                "--detach",
                "-f",
                &replay_path.to_string_lossy(),
                &setup.base_commit,
            ],
        );
        git(
            &replay_path,
            &["apply", "--binary", &diffs[0].patch_path.to_string_lossy()],
        );
        assert_eq!(
            std::fs::read(replay_path.join("asset.bin")).unwrap(),
            blob,
            "the recovered bytes must be identical"
        );
        git(
            repo.path(),
            &[
                "worktree",
                "remove",
                "--force",
                &replay_path.to_string_lossy(),
            ],
        );
    }

    /// LANES_2 / QA-MAJOR — `git branch -D` must not run when the removal FAILED.
    ///
    /// THE USER ACTION: a child commits its work on `subagents/<run>-<i>` and then detaches HEAD
    /// inside its worktree (`git checkout --detach`, a mid-flight rebase, any tool that detaches).
    /// The worktree then cannot be removed — here because it is `git worktree lock`ed, in the
    /// field because it is on a busy mount or holds a file the process cannot unlink. Running
    /// `git branch -D` anyway succeeds, precisely BECAUSE HEAD is detached and git's "branch is
    /// used by a worktree" refusal no longer applies — and the child's commits become unreachable
    /// while the worktree they belong to is still sitting on disk.
    ///
    /// pi runs the branch delete only inside `if (worktreeRemoved)` (`worktree.ts:1253-1263`).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_failed_worktree_removal_never_deletes_the_branch_holding_the_work() {
        let repo = make_real_git_repo();
        let base = tempfile::tempdir().unwrap();
        let options = CreateWorktreesOptions {
            base_dir: Some(base.path().to_string_lossy().into_owned()),
            ..Default::default()
        };
        let setup = create_worktrees(repo.path(), "lockedbranch", 1, Some(&options))
            .await
            .expect("create");
        let worktree = setup.worktrees[0].clone();

        // The child's work: a real commit on the lane branch.
        std::fs::write(worktree.path.join("child.txt"), "child work\n").unwrap();
        git(&worktree.path, &["add", "-A"]);
        git(&worktree.path, &["commit", "-q", "-m", "child work"]);
        let commit = git(&worktree.path, &["rev-parse", "HEAD"]);
        // …and the detach that removes git's own branch-in-use protection.
        git(&worktree.path, &["checkout", "--detach", "--quiet"]);
        // Removal now fails: the worktree is locked.
        git(
            repo.path(),
            &["worktree", "lock", &worktree.path.to_string_lossy()],
        );

        let report = cleanup_worktrees(
            &setup,
            &crate::handoff::WorktreeCleanupIntent::Discard {
                authorization: test_discard_authorization(),
            },
        )
        .await;

        assert!(!report.tasks[0].worktree_removed, "{:?}", report.tasks[0]);
        assert!(
            !report.tasks[0].branch_removed,
            "the branch delete must not even be attempted: {:?}",
            report.tasks[0]
        );
        let listed = git(repo.path(), &["branch", "--list", &worktree.branch]);
        assert!(
            !listed.trim().is_empty(),
            "branch {} must still exist after a failed removal",
            worktree.branch
        );
        assert_eq!(
            git(
                repo.path(),
                &["rev-parse", &format!("{}^{{commit}}", worktree.branch)]
            ),
            commit,
            "and must still point at the child's commit"
        );

        git(
            repo.path(),
            &["worktree", "unlock", &worktree.path.to_string_lossy()],
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn node_modules_symlinked_and_registered_as_synthetic() {
        if cfg!(windows) {
            return;
        }
        let repo = make_real_git_repo();
        let node_modules = repo.path().join("node_modules");
        std::fs::create_dir_all(&node_modules).unwrap();
        std::fs::write(node_modules.join("fixture.txt"), "fixture\n").unwrap();

        let base = tempfile::tempdir().unwrap();
        let options = CreateWorktreesOptions {
            base_dir: Some(base.path().to_string_lossy().into_owned()),
            ..Default::default()
        };
        let setup = create_worktrees(repo.path(), "node-modules", 1, Some(&options))
            .await
            .expect("create");
        assert!(setup.worktrees[0].node_modules_linked);
        assert_eq!(
            setup.worktrees[0].synthetic_paths,
            vec!["node_modules".to_string()]
        );
        let link = setup.worktrees[0].path.join("node_modules");
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        cleanup_worktrees(
            &setup,
            &crate::handoff::WorktreeCleanupIntent::Discard {
                authorization: test_discard_authorization(),
            },
        )
        .await;
    }

    // ---- per-worktree setup hook ----

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn runs_a_repo_relative_setup_hook_and_records_synthetic_paths() {
        if cfg!(windows) {
            return;
        }
        let repo = make_real_git_repo();
        // Commit the hook into the repo so the working tree stays clean (a repo-relative hook path
        // must still resolve against the repo root and run per-worktree).
        let hook_rel_dir = repo.path().join("hooks");
        std::fs::create_dir_all(&hook_rel_dir).unwrap();
        let hook_in_repo = hook_rel_dir.join("hook.sh");
        std::fs::write(
            &hook_in_repo,
            "#!/bin/sh\n[ -n \"$CYRUP_HOOK_WARMUP\" ] && exit 0\nmkdir -p .venv; echo cfg > .venv/pyvenv.cfg; printf '{\"syntheticPaths\":[\".venv\"]}'\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook_in_repo, std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }
        // SUBA-069: this hook is copied into every worktree by git, but the CONTENT is what macOS
        // verifies, so warming the committed copy warms every worktree's copy too.
        warm_hook_exec(&hook_in_repo);
        git(repo.path(), &["add", "-A"]);
        git(repo.path(), &["commit", "-q", "-m", "add hook"]);

        let base = tempfile::tempdir().unwrap();
        let options = CreateWorktreesOptions {
            setup_hook: Some(WorktreeSetupHookConfig {
                hook_path: "hooks/hook.sh".to_string(),
                // SUBA-069: this test's claim is about synthetic-path recording, not about the
                // timeout, so it takes the SHIPPED default (30s, pi
                // `worktree.ts:114 DEFAULT_WORKTREE_SETUP_HOOK_TIMEOUT_MS` @v0.43.0/v0.47.1) rather
                // than a 5s fixture constant that turned scheduling latency into a red.
                timeout_ms: None,
            }),
            base_dir: Some(base.path().to_string_lossy().into_owned()),
            ..Default::default()
        };
        let setup = create_worktrees(repo.path(), "hook-rel", 1, Some(&options))
            .await
            .expect("create");
        assert!(
            setup.worktrees[0]
                .synthetic_paths
                .contains(&".venv".to_string())
        );
        cleanup_worktrees(
            &setup,
            &crate::handoff::WorktreeCleanupIntent::Discard {
                authorization: test_discard_authorization(),
            },
        )
        .await;
    }

    /// SUBA-027 regression: a setup hook that blows through its timeout must be KILLED, matching
    /// upstream `spawnSync(…, { timeout })` (`worktree.ts:323-329`), which kills on expiry.
    /// Before the fix the `Child` lived inside the future `tokio::time::timeout` was racing, so
    /// the elapsed arm dropped the only handle and the hook ran on forever. `exec` in the fixture
    /// is load-bearing: it makes the pid the script publishes the same pid the parent holds, so
    /// this test proves which process was actually signalled rather than reasoning about whether
    /// a given `/bin/sh` forks.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_timed_out_setup_hook_is_killed_not_abandoned() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let hook_path = dir.path().join("hang.sh");
        // `$WARMUP` short-circuits before the pid is published — see the warm-up exec below.
        std::fs::write(
            &hook_path,
            format!(
                "#!/bin/sh\n[ -n \"$WARMUP\" ] && exit 0\necho $$ > '{}'\nexec sleep 300\n",
                pid_file.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755)).unwrap();

        // Pay macOS's first-exec cost BEFORE the clock starts, so the timed run does not race it.
        //
        // macOS charges a one-off verification cost on the first `exec` of a freshly written
        // executable whose exact content it has not seen, and `tempfile::tempdir()` randomizes the
        // path that this script embeds in its own body — so the content is unique on EVERY run and
        // the cost is paid on EVERY run. Measured here: 197-242ms to reach line 2 for unique
        // content (6/6 runs) versus ~0.15-0.23ms once the identical content has been seen. That is
        // why the original `timeout_ms: 200` could never pass — the SIGTERM landed before
        // `echo $$ > pid` ran, so the test failed in its own precondition helper rather than on its
        // actual claim — and why simply raising the budget was not enough either: at 3000ms it
        // still lost under the full suite's parallel load.
        //
        // Warming the cache with the SAME file removes the dominant term instead of guessing a
        // number, leaving only ordinary scheduling jitter for the budget to absorb.
        let _ = std::process::Command::new(&hook_path)
            .env("WARMUP", "1")
            .status();

        // The assertion's meaning is unchanged by the budget: the hook still `exec sleep 300`, so
        // it still blows whatever budget it is given and the timeout arm still fires. The budget
        // only has to be long enough for the hook to publish the pid that proves WHICH process was
        // signalled.
        let hook = ResolvedWorktreeSetupHook {
            hook_path,
            timeout_ms: 3_000,
        };
        let input = WorktreeSetupHookInput {
            version: 1,
            repo_root: dir.path(),
            worktree_path: dir.path(),
            agent_cwd: dir.path(),
            branch: "suba-027",
            index: 0,
            run_id: "suba-027",
            base_commit: "0000000",
            agent: None,
        };

        let err = run_worktree_setup_hook(&hook, &input)
            .await
            .expect_err("a hook that never exits must surface as a timeout");
        assert!(
            err.to_string().contains("timed out"),
            "unexpected error: {err}"
        );

        let pid = wait_for_published_pid(&pid_file, Duration::from_secs(5)).await;
        assert!(
            wait_for_pid_gone(pid, Duration::from_secs(5)).await,
            "setup hook pid {pid} must be gone once the timeout is reported — Node's spawnSync \
             timeout kills the hook, and so must this"
        );
    }

    /// Poll `kill(pid, 0)` until it reports ESRCH, up to `timeout`.
    #[cfg(unix)]
    async fn wait_for_pid_gone(pid: i32, timeout: Duration) -> bool {
        let target = nix::unistd::Pid::from_raw(pid);
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if nix::sys::signal::kill(target, None).is_err() {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Poll for `path` to contain a parseable pid, up to `timeout`.
    #[cfg(unix)]
    async fn wait_for_published_pid(path: &std::path::Path, timeout: Duration) -> i32 {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if let Ok(raw) = std::fs::read_to_string(path)
                && let Ok(pid) = raw.trim().parse::<i32>()
            {
                return pid;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the hook never published its pid to {} within {timeout:?}",
                path.display()
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rejects_bare_command_names_for_setup_hooks() {
        let repo = make_real_git_repo();
        let base = tempfile::tempdir().unwrap();
        let options = CreateWorktreesOptions {
            setup_hook: Some(WorktreeSetupHookConfig {
                hook_path: "node".to_string(),
                timeout_ms: None,
            }),
            base_dir: Some(base.path().to_string_lossy().into_owned()),
            ..Default::default()
        };
        let err = create_worktrees(repo.path(), "hook-bare", 1, Some(&options))
            .await
            .expect_err("bare command rejected");
        let SubagentError::WorktreeSetup(msg) = err else {
            panic!("wrong variant")
        };
        assert!(
            msg.contains("absolute path or a repo-relative path"),
            "{msg}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rejects_tracked_synthetic_paths_from_hook_output() {
        if cfg!(windows) {
            return;
        }
        let repo = make_real_git_repo();
        let (_d, hook) =
            write_hook_script("cat > /dev/null; printf '{\"syntheticPaths\":[\"tracked.txt\"]}'");
        let base = tempfile::tempdir().unwrap();
        let options = CreateWorktreesOptions {
            setup_hook: Some(WorktreeSetupHookConfig {
                hook_path: hook.to_string_lossy().into_owned(),
                // SUBA-069: the shipped default (30s, pi `worktree.ts:114`
                // `DEFAULT_WORKTREE_SETUP_HOOK_TIMEOUT_MS`). This test asserts nothing about the
                // timeout, so it must not carry a 5s budget that machine load can blow.
                timeout_ms: None,
            }),
            base_dir: Some(base.path().to_string_lossy().into_owned()),
            ..Default::default()
        };
        let err = create_worktrees(repo.path(), "hook-tracked", 1, Some(&options))
            .await
            .expect_err("tracked synthetic rejected");
        let SubagentError::WorktreeSetup(msg) = err else {
            panic!("wrong variant")
        };
        assert!(
            msg.contains("cannot mark tracked paths as synthetic"),
            "{msg}"
        );
        // Rollback ran — nothing left under the base dir.
        let remaining: Vec<_> = std::fs::read_dir(base.path()).unwrap().collect();
        assert!(remaining.is_empty(), "rollback must clean up");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn excludes_hook_created_synthetic_files_from_captured_patch() {
        if cfg!(windows) {
            return;
        }
        let repo = make_real_git_repo();
        let (_d, hook) = write_hook_script(
            "cat > /dev/null; printf 'TOKEN=secret\\n' > .env.local; printf '{\"syntheticPaths\":[\".env.local\"]}'",
        );
        let base = tempfile::tempdir().unwrap();
        let options = CreateWorktreesOptions {
            setup_hook: Some(WorktreeSetupHookConfig {
                hook_path: hook.to_string_lossy().into_owned(),
                // SUBA-069: the shipped default (30s, pi `worktree.ts:114`
                // `DEFAULT_WORKTREE_SETUP_HOOK_TIMEOUT_MS`). This test asserts nothing about the
                // timeout, so it must not carry a 5s budget that machine load can blow.
                timeout_ms: None,
            }),
            base_dir: Some(base.path().to_string_lossy().into_owned()),
            ..Default::default()
        };
        let setup = create_worktrees(repo.path(), "hook-diff", 1, Some(&options))
            .await
            .expect("create");
        std::fs::write(
            setup.worktrees[0].path.join("tracked.txt"),
            "modified-by-agent\n",
        )
        .unwrap();
        let diffs = diff_worktrees(
            &setup,
            &["agent-a".to_string()],
            &repo.path().join("hook-diff"),
        )
        .await;
        let patch = std::fs::read_to_string(&diffs[0].patch_path).unwrap();
        assert!(patch.contains("tracked.txt"));
        assert!(
            !patch.contains(".env.local"),
            "synthetic hook file must be excluded: {patch}"
        );
        cleanup_worktrees(
            &setup,
            &crate::handoff::WorktreeCleanupIntent::Discard {
                authorization: test_discard_authorization(),
            },
        )
        .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cleans_up_created_worktrees_when_a_later_hook_setup_fails() {
        if cfg!(windows) {
            return;
        }
        let repo = make_real_git_repo();
        // Fail only for index 1.
        let (_d, hook) = write_hook_script(
            "payload=$(cat); case \"$payload\" in *'\"index\":1'*) echo fail 1>&2; exit 1;; esac; printf '{\"syntheticPaths\":[]}'",
        );
        let base = tempfile::tempdir().unwrap();
        let options = CreateWorktreesOptions {
            setup_hook: Some(WorktreeSetupHookConfig {
                hook_path: hook.to_string_lossy().into_owned(),
                // SUBA-069: the shipped default (30s, pi `worktree.ts:114`
                // `DEFAULT_WORKTREE_SETUP_HOOK_TIMEOUT_MS`). This test asserts nothing about the
                // timeout, so it must not carry a 5s budget that machine load can blow.
                timeout_ms: None,
            }),
            base_dir: Some(base.path().to_string_lossy().into_owned()),
            ..Default::default()
        };
        let err = create_worktrees(repo.path(), "hook-cleanup", 2, Some(&options))
            .await
            .expect_err("second hook fails");
        assert!(matches!(err, SubagentError::WorktreeSetup(_)));
        let branches = git(
            repo.path(),
            &["branch", "--list", "cyrup-parallel-hook-cleanup-*"],
        );
        assert!(
            branches.trim().is_empty(),
            "temp branches must be cleaned up: {branches}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn hook_that_exceeds_timeout_fails_within_the_bound() {
        if cfg!(windows) {
            return;
        }
        let repo = make_real_git_repo();
        let (_d, hook) = write_hook_script("sleep 30");
        let base = tempfile::tempdir().unwrap();
        let options = CreateWorktreesOptions {
            setup_hook: Some(WorktreeSetupHookConfig {
                hook_path: hook.to_string_lossy().into_owned(),
                timeout_ms: Some(200),
            }),
            base_dir: Some(base.path().to_string_lossy().into_owned()),
            ..Default::default()
        };
        let started = std::time::Instant::now();
        let err = create_worktrees(repo.path(), "hook-timeout", 1, Some(&options))
            .await
            .expect_err("timeout");
        assert!(matches!(err, SubagentError::WorktreeSetup(_)));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "must be bounded: {:?}",
            started.elapsed()
        );
    }

    // ---- preview ----

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn previews_expected_worktree_agent_cwd_for_subdirectories() {
        let repo = make_real_git_repo();
        let nested = repo.path().join("packages").join("app");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("index.ts"), "export const v = 1;\n").unwrap();
        git(repo.path(), &["add", "-A"]);
        git(repo.path(), &["commit", "-q", "-m", "nested"]);

        let base = tempfile::tempdir().unwrap();
        let previewed = resolve_expected_worktree_agent_cwd(
            &nested,
            "preview",
            2,
            Some(&base.path().to_string_lossy()),
        )
        .await
        .expect("preview");
        assert_eq!(
            previewed,
            base.path()
                .join("cyrup-worktree-preview-2")
                .join("packages")
                .join("app")
        );
    }

    // ---- legacy compat surface ----

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn setup_worktree_group_compat_returns_agent_cwds() {
        let repo = make_real_git_repo();
        let base = tempfile::tempdir().unwrap();
        let config = WorktreeGroupConfig {
            group_id: "compat",
            worktree_base_dir: Some(base.path()),
            setup_hook: None,
            setup_hook_timeout_ms: None,
        };
        let overrides: Vec<Option<&Path>> = vec![None, None];
        let plan = setup_worktree_group(repo.path(), &overrides, &config)
            .await
            .expect("group");
        assert_eq!(plan.task_count(), 2);
        for a in &plan.assignments {
            assert!(a.path.is_dir());
        }
        let setup = WorktreeSetup {
            cwd: PathBuf::from(git(repo.path(), &["rev-parse", "--show-toplevel"])),
            worktrees: plan
                .assignments
                .iter()
                .map(|a| WorktreeInfo {
                    path: a.path.clone(),
                    agent_cwd: a.path.clone(),
                    branch: a.branch.clone(),
                    index: a.index,
                    node_modules_linked: false,
                    synthetic_paths: Vec::new(),
                })
                .collect(),
            base_commit: plan.base_commit.clone(),
        };
        cleanup_worktrees(
            &setup,
            &crate::handoff::WorktreeCleanupIntent::Discard {
                authorization: test_discard_authorization(),
            },
        )
        .await;
    }

    // ---- the harvest gate (pi `worktree.ts:1197-1231` @v0.68.0) ----

    /// Confine a test's worktrees to a tempdir that is dropped with the test.
    ///
    /// `create_worktrees` otherwise derives the worktree path from the GROUP LABEL under a shared
    /// root, so two runs of the same test collide — and a test whose point is that removal FAILS
    /// leaves debris that poisons every later run. Learned the hard way: the first version of
    /// [`a_failed_removal_does_not_take_the_branch_with_it`] passed once and then failed forever
    /// with "fatal: '/tmp/cyrup-worktree-branch-order-0' already exists".
    fn confined_base(dir: &tempfile::TempDir) -> CreateWorktreesOptions {
        CreateWorktreesOptions {
            base_dir: Some(dir.path().to_string_lossy().into_owned()),
            ..Default::default()
        }
    }

    /// Dirty a worktree the two probes see DIFFERENTLY, so neither alone would do.
    ///
    /// `git status --porcelain` reports both rows; a diff against the base commit reports only
    /// the tracked modification, because an untracked file is in no tree. A gate written on the
    /// base diff alone would therefore delete a worktree whose only work is a brand-new file —
    /// which is the common case for a child that was asked to CREATE something.
    fn dirty_both_ways(worktree: &Path) {
        std::fs::write(worktree.join("file.txt"), "child edited this\n").unwrap();
        std::fs::write(worktree.join("brand-new.txt"), "child created this\n").unwrap();
    }

    /// THE DATA-LOSS TEST.
    ///
    /// The harvest path (`WorktreeCleanupIntent::Preserve`) runs on EVERY successful
    /// `worktree: true` fan-out, with no operator involved. Before the gate at
    /// [`refuse_uncaptured_preserve`] existed, it fell straight through to
    /// `git worktree remove --force` + `git branch -D`, so a child that left uncommitted or
    /// untracked work had that work destroyed on the ordinary SUCCESS path.
    ///
    /// Here the harvest claims no capture at all (`captured_diffs: vec![]`, the honest spelling of
    /// "nothing was captured"), so there is nothing that could represent the work. Both the
    /// worktree and its branch must survive, and the ledger must say why.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_harvest_never_force_removes_a_worktree_whose_work_was_not_captured() {
        let repo = make_real_git_repo();
        let base = tempfile::tempdir().unwrap();
        let setup = create_worktrees(repo.path(), "harvest-gate", 1, Some(&confined_base(&base)))
            .await
            .expect("create");
        let worktree = setup.worktrees[0].path.clone();
        let branch = setup.worktrees[0].branch.clone();
        dirty_both_ways(&worktree);

        let report = cleanup_worktrees(
            &setup,
            &crate::handoff::WorktreeCleanupIntent::Preserve(crate::handoff::PreserveEvidence {
                captured_diffs: Vec::new(),
                handoff_manifest_path: None,
            }),
        )
        .await;

        let task = &report.tasks[0];
        assert_eq!(
            task.preserved,
            Some(true),
            "a worktree holding uncaptured work must be preserved: {task:?}"
        );
        assert!(!task.worktree_removed, "{task:?}");
        assert!(!task.branch_removed, "{task:?}");
        assert!(
            worktree.join("brand-new.txt").exists(),
            "the child's untracked file was destroyed at {}",
            worktree.display()
        );
        assert_eq!(
            std::fs::read_to_string(worktree.join("file.txt")).unwrap(),
            "child edited this\n",
            "the child's uncommitted edit was destroyed"
        );
        assert!(
            git(repo.path(), &["branch", "--list", &branch]).contains(&branch),
            "the branch holding the child's work was deleted"
        );

        // Cleanup: the discard path, which is the one that MAY remove it (authorized).
        cleanup_worktrees(
            &setup,
            &crate::handoff::WorktreeCleanupIntent::Discard {
                authorization: test_discard_authorization(),
            },
        )
        .await;
    }

    /// The gate must not leak worktrees forever: a CLEAN worktree has no work to lose, so the
    /// harvest removes it exactly as before. Without this, "refuse everything" would pass the test
    /// above and quietly turn every fan-out into an orphan-worktree generator.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_harvest_still_removes_a_clean_worktree() {
        let repo = make_real_git_repo();
        let base = tempfile::tempdir().unwrap();
        let setup = create_worktrees(repo.path(), "harvest-clean", 1, Some(&confined_base(&base)))
            .await
            .expect("create");
        let worktree = setup.worktrees[0].path.clone();
        let branch = setup.worktrees[0].branch.clone();

        let report = cleanup_worktrees(
            &setup,
            &crate::handoff::WorktreeCleanupIntent::Preserve(crate::handoff::PreserveEvidence {
                captured_diffs: Vec::new(),
                handoff_manifest_path: None,
            }),
        )
        .await;

        let task = &report.tasks[0];
        assert_ne!(
            task.preserved,
            Some(true),
            "a clean worktree has nothing to protect and must still be reclaimed: {task:?}"
        );
        assert!(task.worktree_removed, "{task:?}");
        assert!(task.branch_removed, "{task:?}");
        assert!(!worktree.exists(), "{}", worktree.display());
        assert!(
            !git(repo.path(), &["branch", "--list", &branch]).contains(&branch),
            "the branch should have been reclaimed with its worktree"
        );
    }

    /// `git branch -D` runs ONLY after the worktree really went away (pi `:1253-1263`).
    ///
    /// A removal that fails leaves the work on disk, and the branch may be the only ref reaching
    /// the commits the child made. git normally refuses to delete a branch that a worktree has
    /// checked out — but a child that detached HEAD inside its worktree removes that protection,
    /// which is exactly the case where the commits matter. Here the worktree directory is made
    /// un-removable by deleting it out from under git and leaving the administrative link, so
    /// `git worktree remove` fails while the branch still exists.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_failed_removal_does_not_take_the_branch_with_it() {
        let repo = make_real_git_repo();
        let base = tempfile::tempdir().unwrap();
        let setup = create_worktrees(repo.path(), "branch-order", 1, Some(&confined_base(&base)))
            .await
            .expect("create");
        let worktree = setup.worktrees[0].path.clone();
        let branch = setup.worktrees[0].branch.clone();

        // Detach HEAD inside the worktree so git's own "branch is checked out" protection is gone,
        // then break the worktree so removal cannot succeed.
        git(&worktree, &["checkout", "-q", "--detach"]);
        std::fs::remove_dir_all(worktree.join(".git")).ok();
        std::fs::write(worktree.join(".git"), "not a gitlink\n").ok();

        let report = cleanup_worktrees(
            &setup,
            &crate::handoff::WorktreeCleanupIntent::Preserve(crate::handoff::PreserveEvidence {
                captured_diffs: Vec::new(),
                handoff_manifest_path: None,
            }),
        )
        .await;

        let task = &report.tasks[0];
        if task.worktree_removed {
            // git managed to remove it anyway; then deleting the branch is upstream's behaviour
            // and there is nothing to assert about ordering.
            return;
        }
        assert!(
            !task.branch_removed,
            "the branch was deleted although the worktree removal failed: {task:?}"
        );
        assert!(
            git(repo.path(), &["branch", "--list", &branch]).contains(&branch),
            "a failed removal must leave the branch reaching the child's commits"
        );
    }
}
