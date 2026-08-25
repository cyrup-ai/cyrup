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
//! - [`cleanup_worktrees`] — best-effort removal of every worktree + branch, then `git worktree
//!   prune`. Called on the **success** path (after harvest) as well as on rollback (C18).
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

/// The raw result of one `git` invocation (pi `GitResult`).
struct GitResult {
    stdout: String,
    stderr: String,
    status: Option<i32>,
}

// =================================================================================================
// git subprocess helpers (pi runGit / runGitChecked)
// =================================================================================================

async fn run_git(cwd: &Path, args: &[&str]) -> Result<GitResult, SubagentError> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .await
        .map_err(SubagentError::Spawn)?;
    Ok(GitResult {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        status: output.status.code(),
    })
}

async fn run_git_checked(cwd: &Path, args: &[&str]) -> Result<String, SubagentError> {
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
fn lexical_normalize(base: &Path, relative: &Path) -> PathBuf {
    let joined = base.join(relative);
    let mut out: Vec<std::path::Component<'_>> = Vec::new();
    for comp in joined.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                match out.last() {
                    Some(std::path::Component::Normal(_)) => {
                        out.pop();
                    }
                    _ => out.push(comp),
                }
            }
            other => out.push(other),
        }
    }
    out.iter().map(|component| component.as_os_str()).collect()
}

/// pi `normalizeComparableCwd`: absolute-resolve then realpath, falling back to the unresolved
/// absolute path when realpath resolution is unavailable.
fn normalize_comparable_cwd(cwd: &Path) -> PathBuf {
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

/// pi `resolveWorktreeBaseDir`.
fn resolve_worktree_base_dir(
    configured_base_dir: Option<&str>,
    repo_root: &Path,
) -> Result<PathBuf, SubagentError> {
    let raw = configured_base_dir
        .map(str::to_string)
        .or_else(|| std::env::var(WORKTREE_DIR_ENV).ok());
    let Some(raw) = raw else {
        return Ok(std::env::temp_dir());
    };

    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(SubagentError::WorktreeSetup(
            "worktree base directory cannot be empty".to_string(),
        ));
    }

    let expanded: PathBuf = trimmed
        .strip_prefix("~/")
        .map_or_else(|| PathBuf::from(trimmed), |rest| crate::paths::home_dir().join(rest));
    let resolved = if expanded.is_absolute() {
        expanded
    } else {
        repo_root.join(expanded)
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
    Ok(if normalized == "." { String::new() } else { normalized })
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

    let expanded: PathBuf = hook_path
        .strip_prefix("~/")
        .map_or_else(|| PathBuf::from(hook_path), |rest| crate::paths::home_dir().join(rest));

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
        SubagentError::WorktreeSetup(format!("failed to serialize worktree setup hook input: {err}"))
    })?;
    let worktree_path = input.worktree_path;

    let mut child = Command::new(&hook.hook_path)
        .current_dir(worktree_path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|err| SubagentError::WorktreeSetup(format!("worktree setup hook failed: {err}")))?;

    let call = async {
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(&payload).await.map_err(SubagentError::Spawn)?;
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
                &["worktree", "remove", "--force", &worktree_path.to_string_lossy()],
            )
            .await;
            let _ = run_git(toplevel, &["branch", "-D", &branch]).await;
            Err(err)
        }
    }
}

/// pi `createWorktrees`: the full synchronous-before-any-spawn setup sequence. On any failure,
/// every worktree created so far is cleaned up before the error propagates.
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
                cleanup_worktrees(&WorktreeSetup {
                    cwd: repo.toplevel.clone(),
                    worktrees,
                    base_commit: repo.base_commit.clone(),
                })
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

fn empty_diff(index: u32, agent: &str, branch: &str, patch_path: &Path) -> WorktreeDiff {
    WorktreeDiff {
        index,
        agent: agent.to_string(),
        branch: branch.to_string(),
        diff_stat: String::new(),
        files_changed: 0,
        insertions: 0,
        deletions: 0,
        patch_path: patch_path.to_path_buf(),
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
    let diff_stat = run_git_checked(
        &worktree.path,
        &["diff", "--cached", "--stat", &setup.base_commit],
    )
    .await?
    .trim()
    .to_string();
    let patch =
        run_git_checked(&worktree.path, &["diff", "--cached", &setup.base_commit]).await?;
    let numstat = run_git_checked(
        &worktree.path,
        &["diff", "--cached", "--numstat", &setup.base_commit],
    )
    .await?;

    std::fs::write(patch_path, &patch).map_err(SubagentError::Spawn)?;

    if patch.trim().is_empty() {
        return Ok(empty_diff(worktree.index, agent, &worktree.branch, patch_path));
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
    })
}

fn write_empty_patch(patch_path: &Path) {
    let _ = std::fs::write(patch_path, "");
}

/// pi `diffWorktrees`: capture one `.patch` per worktree under `diffs_dir`, mapping any per-task
/// capture failure to an empty patch + empty diff rather than failing the whole harvest.
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
            Err(_) => {
                write_empty_patch(&patch_path);
                let idx = u32::try_from(index).unwrap_or(u32::MAX);
                diffs.push(empty_diff(idx, &agent, &worktree.branch, &patch_path));
            }
        }
    }
    diffs
}

/// pi `cleanupWorktrees`: best-effort removal of every worktree + branch (reverse order), then
/// `git worktree prune`. Safe to call on the **success** path after harvest, as well as on
/// rollback.
pub async fn cleanup_worktrees(setup: &WorktreeSetup) {
    for worktree in setup.worktrees.iter().rev() {
        let _ = run_git(
            &setup.cwd,
            &["worktree", "remove", "--force", &worktree.path.to_string_lossy()],
        )
        .await;
        let _ = run_git(&setup.cwd, &["branch", "-D", &worktree.branch]).await;
    }
    let _ = run_git(&setup.cwd, &["worktree", "prune"]).await;
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
    pub worktree_base_dir: &'a Path,
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

/// Legacy plan returned by [`setup_worktree_group`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeGroupPlan {
    /// One assignment per task, in task order.
    pub assignments: Vec<WorktreeAssignment>,
    /// The group's common base commit.
    pub base_commit: String,
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
    let tasks: Vec<(&str, Option<&str>)> = owned_cwds
        .iter()
        .map(|c| ("task", c.as_deref()))
        .collect();
    if let Some(conflict) = find_worktree_task_cwd_conflict(&tasks, repo_cwd) {
        return Err(SubagentError::WorktreeSetup(format_worktree_task_cwd_conflict(
            &conflict, repo_cwd,
        )));
    }

    let setup_hook = config.setup_hook.map(|hook| WorktreeSetupHookConfig {
        hook_path: hook.command.to_string_lossy().into_owned(),
        timeout_ms: config.setup_hook_timeout_ms,
    });
    let options = CreateWorktreesOptions {
        agents: None,
        setup_hook,
        base_dir: Some(config.worktree_base_dir.to_string_lossy().into_owned()),
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
        base_commit: setup.base_commit,
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
        assert!(out.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
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
        assert_eq!(setup.cwd, PathBuf::from(git(repo.path(), &["rev-parse", "--show-toplevel"])));
        for (i, wt) in setup.worktrees.iter().enumerate() {
            assert_eq!(wt.branch, format!("cyrup-parallel-structure-{i}"));
            assert_eq!(wt.index, u32::try_from(i).unwrap());
            assert_eq!(wt.agent_cwd, wt.path);
            assert!(!wt.node_modules_linked);
            assert!(wt.synthetic_paths.is_empty());
            assert!(wt.path.is_dir());
        }
        cleanup_worktrees(&setup).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn create_worktrees_maps_subdirectory_cwd_to_agent_cwd() {
        let repo = make_real_git_repo();
        let nested = repo.path().join("packages").join("app");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("index.ts"), "export const v = 1;\n").unwrap();
        git(repo.path(), &["add", "-A"]);
        git(repo.path(), &["commit", "-q", "-m", "add nested"]);

        let setup = create_worktrees(&nested, "subdir", 1, None).await.expect("create");
        assert_eq!(
            setup.worktrees[0].agent_cwd,
            setup.worktrees[0].path.join("packages").join("app")
        );
        cleanup_worktrees(&setup).await;
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
        assert_eq!(setup.worktrees[0].path, base_dir.join("cyrup-worktree-base-dir-0"));
        assert!(base_dir.exists());
        cleanup_worktrees(&setup).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn create_worktrees_rejects_dirty_repositories() {
        let repo = make_real_git_repo();
        std::fs::write(repo.path().join("tracked.txt"), "dirty\n").unwrap();
        let err = create_worktrees(repo.path(), "dirty", 1, None)
            .await
            .expect_err("dirty rejects");
        let SubagentError::WorktreeSetup(msg) = err else { panic!("wrong variant") };
        assert!(msg.contains("clean git working tree"), "{msg}");
        // No worktree was created.
        let list = git(repo.path(), &["worktree", "list", "--porcelain"]);
        assert_eq!(list.matches("worktree ").count(), 1);
    }

    // ---- cwd-conflict (allow-equal) ----

    #[test]
    fn conflict_allows_omitted_or_matching_task_cwd() {
        let shared = Path::new("/tmp/repo");
        assert!(find_worktree_task_cwd_conflict(
            &[("worker-a", None), ("worker-b", Some("/tmp/repo"))],
            shared
        )
        .is_none());
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
            &[("worker-a", Some("/tmp/repo")), ("worker-b", Some("/tmp/repo/packages/app"))],
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
            std::fs::write(wt.path.join("committed.ts"), format!("export const c{i} = true;\n"))
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
            assert_eq!(diff.files_changed, 3, "3 files per worktree, got {}", diff.files_changed);
            assert!(diff.insertions > 0);
            assert!(diff.patch_path.exists(), "per-task patch file must exist");
            let patch = std::fs::read_to_string(&diff.patch_path).unwrap();
            assert!(patch.contains("committed.ts"));
            assert!(patch.contains("tracked.txt"));
            assert!(patch.contains("new-file.ts"));
            // node_modules symlink was stripped before diffing — never leaks in.
            assert!(!patch.contains("diff --git a/node_modules b/node_modules"), "{patch}");
        }

        let summary = format_worktree_diff_summary(&diffs);
        assert!(summary.contains("=== Worktree Changes ==="));
        assert!(summary.contains("Full patches:"));

        // Success-path cleanup: worktrees and branches are gone (C18).
        let paths: Vec<PathBuf> = setup.worktrees.iter().map(|w| w.path.clone()).collect();
        let branches: Vec<String> = setup.worktrees.iter().map(|w| w.branch.clone()).collect();
        cleanup_worktrees(&setup).await;
        for path in &paths {
            assert!(!path.exists(), "worktree {} must be removed", path.display());
        }
        for branch in &branches {
            let listed = git(repo.path(), &["branch", "--list", branch]);
            assert!(listed.trim().is_empty(), "branch {branch} must be deleted");
        }
        let list = git(repo.path(), &["worktree", "list", "--porcelain"]);
        assert_eq!(list.matches("worktree ").count(), 1, "only primary remains");
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
        assert_eq!(setup.worktrees[0].synthetic_paths, vec!["node_modules".to_string()]);
        let link = setup.worktrees[0].path.join("node_modules");
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        cleanup_worktrees(&setup).await;
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
            std::fs::set_permissions(&hook_in_repo, std::fs::Permissions::from_mode(0o755)).unwrap();
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
        assert!(setup.worktrees[0].synthetic_paths.contains(&".venv".to_string()));
        cleanup_worktrees(&setup).await;
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
        let SubagentError::WorktreeSetup(msg) = err else { panic!("wrong variant") };
        assert!(msg.contains("absolute path or a repo-relative path"), "{msg}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rejects_tracked_synthetic_paths_from_hook_output() {
        if cfg!(windows) {
            return;
        }
        let repo = make_real_git_repo();
        let (_d, hook) = write_hook_script("cat > /dev/null; printf '{\"syntheticPaths\":[\"tracked.txt\"]}'");
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
        let SubagentError::WorktreeSetup(msg) = err else { panic!("wrong variant") };
        assert!(msg.contains("cannot mark tracked paths as synthetic"), "{msg}");
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
        std::fs::write(setup.worktrees[0].path.join("tracked.txt"), "modified-by-agent\n").unwrap();
        let diffs = diff_worktrees(&setup, &["agent-a".to_string()], &repo.path().join("hook-diff")).await;
        let patch = std::fs::read_to_string(&diffs[0].patch_path).unwrap();
        assert!(patch.contains("tracked.txt"));
        assert!(!patch.contains(".env.local"), "synthetic hook file must be excluded: {patch}");
        cleanup_worktrees(&setup).await;
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
        let branches = git(repo.path(), &["branch", "--list", "cyrup-parallel-hook-cleanup-*"]);
        assert!(branches.trim().is_empty(), "temp branches must be cleaned up: {branches}");
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
        assert!(started.elapsed() < Duration::from_secs(5), "must be bounded: {:?}", started.elapsed());
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
            base.path().join("cyrup-worktree-preview-2").join("packages").join("app")
        );
    }

    // ---- legacy compat surface ----

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn setup_worktree_group_compat_returns_agent_cwds() {
        let repo = make_real_git_repo();
        let base = tempfile::tempdir().unwrap();
        let config = WorktreeGroupConfig {
            group_id: "compat",
            worktree_base_dir: base.path(),
            setup_hook: None,
            setup_hook_timeout_ms: None,
        };
        let overrides: Vec<Option<&Path>> = vec![None, None];
        let plan = setup_worktree_group(repo.path(), &overrides, &config).await.expect("group");
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
        cleanup_worktrees(&setup).await;
    }

}
