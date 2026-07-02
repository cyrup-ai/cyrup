//! Git-worktree isolation for `worktree: true` parallel fan-out groups (func-SA §5.3
//! R-SA-060..065; arch-SA §6.4's "Worktree setup ordering" section).
//!
//! # What this module does, and the ordering it enforces
//!
//! For a `ParallelGroup` opted into worktree isolation, every concurrently-spawned child MUST get
//! its own dedicated working directory carved out of the shared repository via `git worktree add`
//! (R-SA-061) — never a shared cwd, which would let sibling children stomp on each other's
//! uncommitted changes. The whole setup sequence is entirely synchronous-before-any-spawn
//! (arch-SA §6.4):
//!
//! 1. [`check_clean_working_tree`] — `git status --porcelain` MUST report empty output in the
//!    shared cwd, or the entire group fails before any worktree (let alone any child process) is
//!    created (R-SA-060). No partial/degraded non-isolated fallback is ever attempted.
//! 2. [`reject_task_level_cwd_overrides`] — every task in the group is scanned for an explicit
//!    `cwd` override, which would defeat isolation; if any is found, the whole group is rejected
//!    (R-SA-062) with an all-tasks-failed result, never a partial run.
//! 3. Only once both of the above pass, [`create_worktrees`] loops `git worktree add <path> -b
//!    <branch> <base_commit>` once per task index (R-SA-061), and — if a hook is configured —
//!    [`run_setup_hook`] invokes it with a bounded timeout (R-SA-063), enforcing the
//!    synthetic-path safety rail (R-SA-064) against its response.
//!
//! Any failure at any point in step 3 triggers [`cleanup_worktrees`] (best-effort per worktree,
//! R-SA-065) over whatever worktrees were already created, and the entire group aborts with zero
//! children spawned — this module never hands back a partially-isolated group.
//!
//! # Why this shells out to a real `git` subprocess, never a Rust git library
//!
//! Per func-SA §9 item 15, `git worktree`/`git status` invocation deliberately shells out via
//! subprocess — consistent with this crate's subprocess-first design (func-SA §1.1) and exact
//! stderr/stdout parity with the real `git` CLI's own error messages — rather than going through a
//! Rust git library (e.g. `gix`, which `cyrup-resources` uses for its own clone/checkout needs;
//! `git worktree` specifically has no mature `gix` equivalent as of this writing). This is a
//! deliberate, documented choice, not an oversight: unlike `cyrup-resources`' clone/fetch/checkout
//! needs (which gix serves well), `git worktree add`/`git status --porcelain` are thin,
//! well-specified CLI surfaces this crate calls directly.
//!
//! # Scope note: `SingleStep`/`RunnerStep` integration is a later phase
//!
//! `spawn::chain_graph::RunnerStep`/`ParallelGroup`/`SingleStep` (the real discriminated-union
//! chain-graph types, arch-SA §2.2 Phase 3) are not yet built out in this crate (as of this file).
//! This module is therefore written against a minimal, self-contained view of what it actually
//! needs from a group of tasks — a per-task optional cwd override
//! ([`reject_task_level_cwd_overrides`]'s `task_cwd_overrides: &[Option<&Path>]` parameter) and a
//! task count ([`WorktreeGroupPlan::task_count`]) — rather than depending on the not-yet-existing
//! `SingleStep` type. Once `spawn::chain_graph` lands, its fan-out driver is expected to call
//! straight into [`setup_worktree_group`], passing each `SingleStep`'s own `cwd: Option<PathBuf>`
//! field as that slice; no change to this module's own logic is anticipated.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use crate::error::SubagentError;

/// Default bound on how long the optional setup hook may run before its worktree group is failed
/// (R-SA-063: "target 30000ms, if unset").
pub const DEFAULT_HOOK_TIMEOUT: Duration = Duration::from_millis(30_000);

/// A configured worktree setup hook: an external command invoked once per worktree group, given a
/// JSON payload on stdin and expected to answer with a JSON payload on stdout (R-SA-063).
///
/// Mirrors func-SA §4.7's `HookSpec` data-model entry and arch-SA §3.8's
/// `registration::HookSpec` shape exactly; defined locally in this module (rather than imported
/// from `crate::registration`) because `registration::HookSpec` has not yet been declared as of
/// this file (`registration/mod.rs` is still a doc-comment-only stub in this crate's current
/// build-out) — once it lands, the two shapes are expected to be identical and this module's own
/// definition can be replaced by a type alias without changing any call site's behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookSpec {
    /// The executable to invoke.
    pub command: PathBuf,
    /// Arguments passed to `command`, before the JSON-on-stdin payload.
    pub args: Vec<String>,
}

/// The JSON payload written to the hook's stdin (R-SA-063 target shape:
/// `{ worktree_paths: [...], base_commit, group_id }`).
#[derive(Debug, Clone, serde::Serialize)]
struct HookRequest<'a> {
    worktree_paths: &'a [PathBuf],
    base_commit: &'a str,
    group_id: &'a str,
}

/// The JSON payload expected back on the hook's stdout (R-SA-063 target shape:
/// `{ ok: bool, synthetic_paths: Option<[{ worktree_index, path }]>, error: Option<String> }`).
#[derive(Debug, Clone, serde::Deserialize)]
struct HookResponse {
    ok: bool,
    #[serde(default)]
    synthetic_paths: Option<Vec<SyntheticPathEntry>>,
    #[serde(default)]
    error: Option<String>,
}

/// One `syntheticPaths` entry from a hook response: a path (relative to its worktree's root) the
/// hook declares should be excluded from that worktree's diff — e.g. a lockfile the hook itself
/// regenerated as setup scaffolding, not part of the agent's real work (R-SA-064).
#[derive(Debug, Clone, serde::Deserialize)]
struct SyntheticPathEntry {
    worktree_index: u32,
    path: PathBuf,
}

/// One concurrently-spawned child's dedicated worktree assignment (func-SA §4.4's
/// `WorktreeAssignment`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeAssignment {
    /// The dedicated worktree directory this task's child MUST use as its `cwd` (R-SA-061) —
    /// never a shared cwd.
    pub path: PathBuf,
    /// The branch `git worktree add -b <branch>` created for this worktree.
    pub branch: String,
    /// The commit every worktree in the group was created from (`git status --porcelain`'s clean
    /// check and `git worktree add ... HEAD`'s resolution both happen against this same commit,
    /// so every sibling worktree in a group starts from an identical base).
    pub base_commit: String,
    /// This task's position within the group (0-based) — the same index that both
    /// `HookRequest.worktree_paths` and a `SyntheticPathEntry.worktree_index` reference.
    pub index: u32,
    /// Paths (relative to `path`) the setup hook declared as synthetic (R-SA-064) — excluded from
    /// this worktree's diff by whatever downstream consumer computes one. Empty when no hook is
    /// configured, or when the hook declared none for this index.
    pub synthetic_paths: Vec<PathBuf>,
}

/// Result of [`setup_worktree_group`]: every assignment plus the base commit the whole group was
/// cut from, for callers that want to log/report it independently of any one assignment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeGroupPlan {
    /// One assignment per task, in original task order (index 0..N).
    pub assignments: Vec<WorktreeAssignment>,
    /// The commit the whole group was branched from.
    pub base_commit: String,
}

impl WorktreeGroupPlan {
    /// Number of tasks (and therefore worktrees) in this plan.
    #[must_use]
    pub fn task_count(&self) -> usize {
        self.assignments.len()
    }
}

// -------------------------------------------------------------------------------------------
// Step 1 (R-SA-060): dirty-working-tree precondition, BEFORE any worktree or child is created.
// -------------------------------------------------------------------------------------------

/// Verify `git status --porcelain` reports an empty (clean) working tree in `repo_cwd` (R-SA-060).
///
/// This is the FIRST check in the whole worktree-setup sequence and MUST run — and MUST be
/// observed to fail the entire group — before any `git worktree add` invocation and before any
/// subagent child process is spawned. There is no partial/degraded non-isolated fallback: a dirty
/// tree fails the group outright, it never silently falls back to running children against the
/// shared, unisolated cwd.
///
/// # Errors
///
/// Returns [`SubagentError::WorktreeSetup`] if:
/// - the `git` invocation itself fails to spawn or exits nonzero (surfacing `git`'s own stderr
///   verbatim, per this module's "exact stderr/stdout parity" design note), or
/// - `git status --porcelain` reports ANY non-empty output (the tree is dirty).
pub async fn check_clean_working_tree(repo_cwd: &Path) -> Result<(), SubagentError> {
    let output = Command::new("git")
        .arg("status")
        .arg("--porcelain")
        .current_dir(repo_cwd)
        .output()
        .await
        .map_err(SubagentError::Spawn)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(SubagentError::WorktreeSetup(format!(
            "git status --porcelain failed in {}: {}",
            repo_cwd.display(),
            stderr.trim()
        )));
    }

    if !output.stdout.is_empty() {
        let dirty = String::from_utf8_lossy(&output.stdout);
        return Err(SubagentError::WorktreeSetup(format!(
            "working tree at {} is not clean (git status --porcelain reported {} line(s)); \
             worktree isolation requires a clean tree, no partial/degraded fallback is \
             attempted:\n{}",
            repo_cwd.display(),
            dirty.lines().count(),
            dirty.trim()
        )));
    }

    Ok(())
}

// -------------------------------------------------------------------------------------------
// Step 2 (R-SA-062): reject any task-level cwd override before any worktree is created.
// -------------------------------------------------------------------------------------------

/// Reject the whole group if any task explicitly set its own `cwd` (R-SA-062).
///
/// A per-task `cwd` override would defeat worktree isolation outright (the child would run
/// against that explicit path instead of its dedicated worktree), so this check MUST run before
/// any `git worktree add` call — producing an all-tasks-failed result rather than partially
/// running the tasks that did not set an override.
///
/// `task_cwd_overrides` is one entry per task in the group, `Some(path)` when that task
/// explicitly declared its own cwd, `None` otherwise (see this module's header doc for why this
/// takes a plain slice rather than a `SingleStep` — that real type is a later build-out phase).
///
/// # Errors
///
/// Returns [`SubagentError::WorktreeSetup`] naming every offending task index if one or more
/// entries are `Some`.
pub fn reject_task_level_cwd_overrides(
    task_cwd_overrides: &[Option<&Path>],
) -> Result<(), SubagentError> {
    let offenders: Vec<String> = task_cwd_overrides
        .iter()
        .enumerate()
        .filter_map(|(index, cwd)| {
            cwd.map(|path| format!("task[{index}]={}", path.display()))
        })
        .collect();

    if offenders.is_empty() {
        return Ok(());
    }

    Err(SubagentError::WorktreeSetup(format!(
        "worktree: true groups cannot honor per-task cwd overrides (would defeat isolation); \
         reject the whole group before any worktree is created: {}",
        offenders.join(", ")
    )))
}

// -------------------------------------------------------------------------------------------
// Step 3a (R-SA-061): one worktree per concurrent child, from a common base commit.
// -------------------------------------------------------------------------------------------

/// Resolve the commit hash `HEAD` currently points at in `repo_cwd` — the single common base
/// commit every worktree in the group is created from (R-SA-061's `... HEAD` target, pinned to
/// one concrete hash up front so every sibling worktree is provably cut from the identical base
/// even if something else touches the shared repo's `HEAD` mid-setup).
async fn resolve_base_commit(repo_cwd: &Path) -> Result<String, SubagentError> {
    let output = Command::new("git")
        .arg("rev-parse")
        .arg("HEAD")
        .current_dir(repo_cwd)
        .output()
        .await
        .map_err(SubagentError::Spawn)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(SubagentError::WorktreeSetup(format!(
            "git rev-parse HEAD failed in {}: {}",
            repo_cwd.display(),
            stderr.trim()
        )));
    }

    let commit = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if commit.is_empty() {
        return Err(SubagentError::WorktreeSetup(format!(
            "git rev-parse HEAD returned empty output in {}",
            repo_cwd.display()
        )));
    }
    Ok(commit)
}

/// Create exactly one worktree via `git worktree add <path> -b <branch> <base_commit>` (R-SA-061).
///
/// `branch` MUST be unique within the repository (typical callers derive it from the group id and
/// task index, e.g. `subagent/<group_id>/<index>`) — `git worktree add -b` itself fails loudly if
/// the branch already exists, which this function surfaces verbatim rather than silently
/// resolving.
///
/// # Errors
///
/// Returns [`SubagentError::WorktreeSetup`] if the `git worktree add` invocation fails to spawn or
/// exits nonzero, surfacing `git`'s own stderr.
async fn create_one_worktree(
    repo_cwd: &Path,
    path: &Path,
    branch: &str,
    base_commit: &str,
) -> Result<(), SubagentError> {
    let output = Command::new("git")
        .arg("worktree")
        .arg("add")
        .arg(path)
        .arg("-b")
        .arg(branch)
        .arg(base_commit)
        .current_dir(repo_cwd)
        .output()
        .await
        .map_err(SubagentError::Spawn)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(SubagentError::WorktreeSetup(format!(
            "git worktree add {} -b {branch} {base_commit} failed: {}",
            path.display(),
            stderr.trim()
        )));
    }
    Ok(())
}

/// Remove exactly one worktree (and its branch) — the unit of work [`cleanup_worktrees`] applies
/// best-effort, per-worktree (R-SA-065).
///
/// `git worktree remove --force` is used (rather than a plain `remove`) because a worktree
/// abandoned mid-setup (e.g. the group failed after this one was created but before a sibling
/// was) may still have the working-directory-only state `git` would otherwise refuse to discard
/// without `--force` (an untracked file the setup hook wrote, for instance) — cleanup here is
/// explicitly best-effort disposal, not a preservation-preferring operation. The branch is deleted
/// afterward (`git branch -D`) so a repeatedly-failing/retried group does not accumulate orphaned
/// branches; branch deletion failure (e.g. the worktree removal already implicitly cleaned it up
/// on some git versions) is swallowed, matching this function's own best-effort contract.
///
/// # Errors
///
/// Returns `Err` only if the `git worktree remove` invocation itself fails to spawn or exits
/// nonzero — branch-deletion failures are always swallowed (see above). Callers
/// ([`cleanup_worktrees`]) are expected to collect, not propagate, this `Err` so one worktree's
/// cleanup failure never blocks its siblings' cleanup (R-SA-065).
async fn remove_one_worktree(
    repo_cwd: &Path,
    assignment: &WorktreeAssignment,
) -> Result<(), SubagentError> {
    let output = Command::new("git")
        .arg("worktree")
        .arg("remove")
        .arg("--force")
        .arg(&assignment.path)
        .current_dir(repo_cwd)
        .output()
        .await
        .map_err(SubagentError::Spawn)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(SubagentError::WorktreeSetup(format!(
            "git worktree remove --force {} failed: {}",
            assignment.path.display(),
            stderr.trim()
        )));
    }

    // Best-effort branch cleanup — deliberately swallowed (see doc comment above).
    let _ = Command::new("git")
        .arg("branch")
        .arg("-D")
        .arg(&assignment.branch)
        .current_dir(repo_cwd)
        .output()
        .await;

    Ok(())
}

/// Best-effort cleanup of every worktree in `assignments` (R-SA-065): one worktree's removal
/// failure MUST NOT prevent the rest from being attempted. Returns the list of `(index, error)`
/// pairs for whichever removals failed, so the caller can log/report them without the cleanup
/// pass itself aborting partway through.
///
/// This is deliberately infallible at the function-signature level (it never returns `Result`)
/// precisely because R-SA-065 makes cleanup a best-effort operation, never one whose own failure
/// should propagate as if it were a setup failure — callers that need to know cleanup was fully
/// successful inspect the returned `Vec`'s emptiness themselves.
pub async fn cleanup_worktrees(
    repo_cwd: &Path,
    assignments: &[WorktreeAssignment],
) -> Vec<(u32, SubagentError)> {
    let mut failures = Vec::new();
    for assignment in assignments {
        if let Err(err) = remove_one_worktree(repo_cwd, assignment).await {
            tracing::warn!(
                index = assignment.index,
                path = %assignment.path.display(),
                error = %err,
                "best-effort worktree cleanup failed for one sibling (R-SA-065); continuing with \
                 the rest of the group"
            );
            failures.push((assignment.index, err));
        }
    }
    failures
}

// -------------------------------------------------------------------------------------------
// Step 3b (R-SA-063/064): optional setup hook, JSON stdin/stdout contract, bounded timeout.
// -------------------------------------------------------------------------------------------

/// Invoke the configured worktree setup hook once for the whole group (R-SA-063).
///
/// Writes `req` as a single JSON document to the hook's stdin (then closes stdin so a
/// well-behaved hook sees EOF and can proceed to respond), reads its ENTIRE stdout, and parses
/// that as a single JSON [`HookResponse`] document — bounded by `timeout_ms` end-to-end (spawn
/// through response-read), falling back to [`DEFAULT_HOOK_TIMEOUT`] when `timeout_ms` is `None`
/// exactly as R-SA-063 specifies ("falling back to a fixed default, target 30000ms, if unset").
///
/// A hook that exceeds its timeout, exits nonzero, or fails to spawn at all is folded into a
/// single `SubagentError::WorktreeSetup` — from the caller's perspective (`setup_worktree_group`)
/// every one of these failure modes has the identical consequence: fail the entire worktree group
/// before any subagent child is spawned (R-SA-063's own text). A hook that spawns, runs to
/// completion within the timeout, but answers `{"ok": false, "error": "..."}` is likewise folded
/// into the same error variant, carrying the hook's own `error` string forward verbatim so the
/// orchestrator's surfaced failure reason is the hook author's, not a generic message.
///
/// # Errors
///
/// Returns [`SubagentError::WorktreeSetup`] on any of: spawn failure, request-serialization
/// failure (unexpected — `HookRequest` is a plain, always-serializable struct), timeout, nonzero
/// exit, malformed/non-JSON stdout, or an explicit `{"ok": false, ...}` response.
async fn run_setup_hook(
    hook: &HookSpec,
    req: &HookRequest<'_>,
    timeout_ms: Option<u64>,
) -> Result<HookResponse, SubagentError> {
    let timeout = timeout_ms.map_or(DEFAULT_HOOK_TIMEOUT, Duration::from_millis);

    let call = async {
        let payload = serde_json::to_vec(req).map_err(|err| {
            SubagentError::WorktreeSetup(format!(
                "failed to serialize worktree setup hook request: {err}"
            ))
        })?;

        let mut child = Command::new(&hook.command)
            .args(&hook.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(SubagentError::Spawn)?;

        let mut stdin = child.stdin.take().ok_or_else(|| {
            SubagentError::WorktreeSetup("worktree setup hook stdin was not piped".to_string())
        })?;
        stdin.write_all(&payload).await.map_err(SubagentError::Spawn)?;
        stdin.shutdown().await.map_err(SubagentError::Spawn)?;
        drop(stdin); // close our end so the hook reliably observes EOF on its stdin

        let mut stdout = child.stdout.take().ok_or_else(|| {
            SubagentError::WorktreeSetup("worktree setup hook stdout was not piped".to_string())
        })?;
        let mut stdout_buf = Vec::new();
        stdout
            .read_to_end(&mut stdout_buf)
            .await
            .map_err(SubagentError::Spawn)?;

        let status = child.wait().await.map_err(SubagentError::Spawn)?;

        if !status.success() {
            let mut stderr_buf = Vec::new();
            if let Some(mut stderr) = child.stderr.take() {
                let _ = stderr.read_to_end(&mut stderr_buf).await;
            }
            return Err(SubagentError::WorktreeSetup(format!(
                "worktree setup hook {} exited nonzero ({status}): {}",
                hook.command.display(),
                String::from_utf8_lossy(&stderr_buf).trim()
            )));
        }

        let response: HookResponse = serde_json::from_slice(&stdout_buf).map_err(|err| {
            SubagentError::WorktreeSetup(format!(
                "worktree setup hook {} produced non-JSON/malformed stdout: {err}",
                hook.command.display()
            ))
        })?;

        if !response.ok {
            return Err(SubagentError::WorktreeSetup(format!(
                "worktree setup hook {} reported failure: {}",
                hook.command.display(),
                response.error.as_deref().unwrap_or("(no error message)")
            )));
        }

        Ok(response)
    };

    match tokio::time::timeout(timeout, call).await {
        Ok(result) => result,
        Err(_elapsed) => Err(SubagentError::WorktreeSetup(format!(
            "worktree setup hook {} exceeded its {}ms timeout",
            hook.command.display(),
            timeout.as_millis()
        ))),
    }
}

/// Validate a hook's declared `synthetic_paths` against the safety rail (R-SA-064) and fold them
/// into each [`WorktreeAssignment`]'s own `synthetic_paths` field.
///
/// Two independent checks, either of which fails the ENTIRE setup (not just the offending entry):
/// 1. Every declared path MUST be relative to its worktree root — an absolute path is rejected
///    outright (a hook has no legitimate reason to declare a synthetic path outside the worktree
///    it was told about).
/// 2. A path that names a *tracked* git file (i.e. `git ls-files` inside that worktree reports it)
///    MUST fail setup rather than silently excluding real work from the diff — the whole point of
///    this rail is that a hook cannot use "synthetic" to quietly hide committed/tracked changes.
///
/// # Errors
///
/// Returns [`SubagentError::WorktreeSetup`] if any declared path is absolute, references an
/// out-of-range `worktree_index`, or names a tracked file.
async fn apply_synthetic_paths(
    assignments: &mut [WorktreeAssignment],
    synthetic_paths: Vec<SyntheticPathEntry>,
) -> Result<(), SubagentError> {
    for entry in synthetic_paths {
        if entry.path.is_absolute() {
            return Err(SubagentError::WorktreeSetup(format!(
                "worktree setup hook declared an absolute synthetic path {} for worktree_index \
                 {} — synthetic paths must be relative to the worktree root (R-SA-064)",
                entry.path.display(),
                entry.worktree_index
            )));
        }

        let assignment = assignments
            .iter_mut()
            .find(|a| a.index == entry.worktree_index)
            .ok_or_else(|| {
                SubagentError::WorktreeSetup(format!(
                    "worktree setup hook declared a synthetic path for out-of-range \
                     worktree_index {}",
                    entry.worktree_index
                ))
            })?;

        if is_tracked_file(&assignment.path, &entry.path).await? {
            return Err(SubagentError::WorktreeSetup(format!(
                "worktree setup hook marked tracked file {} (worktree_index {}) as synthetic — \
                 marking a TRACKED git file as synthetic must fail setup rather than silently \
                 excluding real work from the diff (R-SA-064)",
                entry.path.display(),
                entry.worktree_index
            )));
        }

        assignment.synthetic_paths.push(entry.path);
    }
    Ok(())
}

/// Whether `relative_path` (relative to `worktree_path`) is tracked by git in that worktree —
/// the R-SA-064 safety-rail check `apply_synthetic_paths` runs against every hook-declared
/// synthetic path.
///
/// Uses `git ls-files --error-unmatch` — the standard, exact-match way to ask "does git track
/// this exact path" (as opposed to `git ls-files <path>` alone, which can match unexpectedly
/// against pathspec-glob semantics for some inputs); a nonzero exit means untracked (not an
/// error condition for this function — that is the expected outcome for a genuinely synthetic
/// path), while a spawn failure is a real error since it means the check itself could not run.
async fn is_tracked_file(worktree_path: &Path, relative_path: &Path) -> Result<bool, SubagentError> {
    let output = Command::new("git")
        .arg("ls-files")
        .arg("--error-unmatch")
        .arg(relative_path)
        .current_dir(worktree_path)
        .output()
        .await
        .map_err(SubagentError::Spawn)?;
    Ok(output.status.success())
}

// -------------------------------------------------------------------------------------------
// Orchestration: the whole synchronous-before-any-spawn sequence (arch-SA §6.4).
// -------------------------------------------------------------------------------------------

/// Everything [`setup_worktree_group`] needs beyond the shared repo cwd and task count.
#[derive(Debug, Clone)]
pub struct WorktreeGroupConfig<'a> {
    /// A stable id for this fan-out group (e.g. a chain-step id or a freshly minted UUID) — used
    /// both to derive unique worktree/branch paths and as `HookRequest.group_id` (R-SA-063).
    pub group_id: &'a str,
    /// Directory new worktrees are created under (typically
    /// `SubagentExtensionConfig.worktree_base_dir`, or a `std::env::temp_dir()`-rooted default
    /// when unset) — kept separate from `repo_cwd` since worktrees are commonly placed outside
    /// the primary checkout to avoid polluting its own directory listing.
    pub worktree_base_dir: &'a Path,
    /// The optional setup hook (R-SA-063); `None` skips step 3b entirely.
    pub setup_hook: Option<&'a HookSpec>,
    /// Bound on the setup hook's total runtime (R-SA-063); `None` falls back to
    /// [`DEFAULT_HOOK_TIMEOUT`].
    pub setup_hook_timeout_ms: Option<u64>,
}

/// Run the complete, synchronous-before-any-spawn worktree-group setup sequence (R-SA-060..064),
/// in the exact order arch-SA §6.4 specifies.
///
/// `repo_cwd` is the shared repository working directory the group's tasks would otherwise all
/// run against. `task_cwd_overrides` is one entry per task (see this module's header doc for why
/// this is a plain slice rather than a `SingleStep` list).
///
/// On ANY failure at any step, whatever worktrees were already created by THIS call are cleaned
/// up (best-effort, R-SA-065) before the error is returned — callers never observe a partially
/// set up group and never need to run their own cleanup pass over a failed [`setup_worktree_group`]
/// call's partial state.
///
/// # Errors
///
/// Returns [`SubagentError::WorktreeSetup`] (or [`SubagentError::Spawn`] for a `git`/hook
/// subprocess I/O failure) if: the working tree is dirty (R-SA-060), any task set an explicit cwd
/// (R-SA-062), any `git worktree add` invocation fails, the setup hook fails/times out/rejects
/// (R-SA-063), or a hook-declared synthetic path fails the safety rail (R-SA-064).
pub async fn setup_worktree_group(
    repo_cwd: &Path,
    task_cwd_overrides: &[Option<&Path>],
    config: &WorktreeGroupConfig<'_>,
) -> Result<WorktreeGroupPlan, SubagentError> {
    // Step 1 (R-SA-060): dirty-tree precondition, before ANYTHING else.
    check_clean_working_tree(repo_cwd).await?;

    // Step 2 (R-SA-062): reject any task-level cwd override, still before any worktree exists.
    reject_task_level_cwd_overrides(task_cwd_overrides)?;

    let base_commit = resolve_base_commit(repo_cwd).await?;

    // Step 3a (R-SA-061): one worktree per task, from the common base commit resolved above.
    let mut assignments = Vec::with_capacity(task_cwd_overrides.len());
    for index in 0..task_cwd_overrides.len() {
        let index_u32 = u32::try_from(index).unwrap_or(u32::MAX);
        let branch = format!("subagent/{}/{index_u32}", config.group_id);
        let path = config
            .worktree_base_dir
            .join(format!("{}-{index_u32}", config.group_id));

        if let Err(err) = create_one_worktree(repo_cwd, &path, &branch, &base_commit).await {
            // Roll back every worktree created so far in THIS call (R-SA-065, best-effort) before
            // propagating — the group must abort with zero children spawned and zero leftover
            // half-set-up worktrees from this attempt.
            cleanup_worktrees(repo_cwd, &assignments).await;
            return Err(err);
        }

        assignments.push(WorktreeAssignment {
            path,
            branch,
            base_commit: base_commit.clone(),
            index: index_u32,
            synthetic_paths: Vec::new(),
        });
    }

    // Step 3b (R-SA-063/064): optional setup hook, only after every worktree above exists.
    if let Some(hook) = config.setup_hook {
        let worktree_paths: Vec<PathBuf> = assignments.iter().map(|a| a.path.clone()).collect();
        let req = HookRequest {
            worktree_paths: &worktree_paths,
            base_commit: &base_commit,
            group_id: config.group_id,
        };

        let response = match run_setup_hook(hook, &req, config.setup_hook_timeout_ms).await {
            Ok(response) => response,
            Err(err) => {
                cleanup_worktrees(repo_cwd, &assignments).await;
                return Err(err);
            }
        };

        if let Some(synthetic_paths) = response.synthetic_paths
            && let Err(err) = apply_synthetic_paths(&mut assignments, synthetic_paths).await
        {
            cleanup_worktrees(repo_cwd, &assignments).await;
            return Err(err);
        }
    }

    Ok(WorktreeGroupPlan {
        assignments,
        base_commit,
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

    /// Create a real, throwaway git repository with one committed file, returning its directory.
    /// Mirrors `crates/cyrup-resources/tests/resources.rs`'s own `make_local_git_repo` helper —
    /// this crate spawns real `git` subprocesses in tests rather than mocking git behavior, per
    /// this codebase's standing convention.
    fn make_real_git_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("real tempdir");
        let run = |args: &[&str]| {
            let status = StdCommand::new("git")
                .current_dir(dir.path())
                .args(args)
                .status()
                .expect("git spawns");
            assert!(status.success(), "git {args:?} must succeed in the test fixture");
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "Test"]);
        std::fs::write(dir.path().join("README.md"), "hello\n").expect("seed file");
        run(&["add", "-A"]);
        run(&["commit", "-q", "-m", "init"]);
        dir
    }

    fn default_config<'a>(group_id: &'a str, worktree_base_dir: &'a Path) -> WorktreeGroupConfig<'a> {
        WorktreeGroupConfig {
            group_id,
            worktree_base_dir,
            setup_hook: None,
            setup_hook_timeout_ms: None,
        }
    }

    // ---- R-SA-060: dirty tree rejects before any child spawns ----

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_clean_tree_passes_the_precondition_check() {
        let repo = make_real_git_repo();
        check_clean_working_tree(repo.path())
            .await
            .expect("a freshly committed repo must be reported clean");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_dirty_tree_is_rejected_by_the_precondition_check_alone() {
        let repo = make_real_git_repo();
        std::fs::write(repo.path().join("uncommitted.txt"), "dirty").expect("dirty the tree");

        let err = check_clean_working_tree(repo.path())
            .await
            .expect_err("an untracked file must make the tree dirty");
        assert!(matches!(err, SubagentError::WorktreeSetup(_)));
    }

    /// The real, end-to-end proof this task asks for: a dirty tree causes
    /// [`setup_worktree_group`] to fail WITHOUT creating a single worktree — verified by
    /// asserting no worktree directory (or `git worktree list` entry beyond the primary checkout)
    /// exists afterward, i.e. nothing that would have become a spawned child's cwd was ever
    /// created.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dirty_tree_rejects_the_whole_group_before_any_worktree_or_child_is_created() {
        let repo = make_real_git_repo();
        std::fs::write(repo.path().join("uncommitted.txt"), "dirty").expect("dirty the tree");

        let worktree_base = tempfile::tempdir().expect("worktree base dir");
        let config = default_config("group-dirty", worktree_base.path());

        let overrides: Vec<Option<&Path>> = vec![None, None, None];
        let err = setup_worktree_group(repo.path(), &overrides, &config)
            .await
            .expect_err("a dirty tree must reject the entire group");
        assert!(matches!(err, SubagentError::WorktreeSetup(_)));

        // Zero worktrees were created: `git worktree list` reports only the primary checkout.
        let list = StdCommand::new("git")
            .current_dir(repo.path())
            .args(["worktree", "list", "--porcelain"])
            .output()
            .expect("git worktree list runs");
        let list_text = String::from_utf8_lossy(&list.stdout);
        assert_eq!(
            list_text.matches("worktree ").count(),
            1,
            "only the primary checkout must be listed — no worktree was created before the \
             dirty-tree rejection, got:\n{list_text}"
        );

        // And nothing was materialized under the worktree base dir either.
        let created: Vec<_> = std::fs::read_dir(worktree_base.path())
            .expect("worktree base dir is readable")
            .collect();
        assert!(
            created.is_empty(),
            "no worktree directories should exist under the worktree base dir at all"
        );
    }

    // ---- R-SA-062: task-level cwd override rejects the whole group ----

    #[test]
    fn no_overrides_passes() {
        let overrides: Vec<Option<&Path>> = vec![None, None];
        reject_task_level_cwd_overrides(&overrides).expect("no overrides must pass");
    }

    #[test]
    fn a_single_task_level_cwd_override_rejects_the_whole_group() {
        let explicit = PathBuf::from("/some/explicit/cwd");
        let overrides: Vec<Option<&Path>> = vec![None, Some(explicit.as_path()), None];
        let err = reject_task_level_cwd_overrides(&overrides)
            .expect_err("any explicit cwd override must reject the whole group");
        let SubagentError::WorktreeSetup(message) = err else {
            panic!("expected WorktreeSetup, got a different variant");
        };
        assert!(
            message.contains("task[1]"),
            "the offending task index must be named in the error: {message}"
        );
    }

    // ---- R-SA-061: N concurrent tasks get N distinct real worktree paths ----

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn n_tasks_get_n_distinct_real_worktree_directories_each_with_a_dedicated_cwd() {
        let repo = make_real_git_repo();
        let worktree_base = tempfile::tempdir().expect("worktree base dir");
        let config = default_config("group-fanout", worktree_base.path());

        const N: usize = 4;
        let overrides: Vec<Option<&Path>> = vec![None; N];
        let plan = setup_worktree_group(repo.path(), &overrides, &config)
            .await
            .expect("a clean tree with no cwd overrides must succeed");

        assert_eq!(plan.task_count(), N, "one assignment per task");

        // Every assignment's path is a real, existing, DISTINCT directory on disk.
        let mut seen = std::collections::HashSet::new();
        for assignment in &plan.assignments {
            assert!(
                assignment.path.is_dir(),
                "assignment path {} must be a real directory on disk",
                assignment.path.display()
            );
            assert!(
                seen.insert(assignment.path.clone()),
                "assignment path {} was assigned to more than one task — must be distinct",
                assignment.path.display()
            );
            // Each worktree is a genuine, independent git working tree rooted at HEAD's content.
            assert!(
                assignment.path.join("README.md").exists(),
                "the worktree at {} must contain the checked-out base-commit content",
                assignment.path.display()
            );
        }
        assert_eq!(seen.len(), N, "N distinct worktree paths for N tasks");

        // `git worktree list` independently confirms N+1 entries (N worktrees + the primary
        // checkout) exist at the OS/git level, not merely in this function's return value.
        let list = StdCommand::new("git")
            .current_dir(repo.path())
            .args(["worktree", "list", "--porcelain"])
            .output()
            .expect("git worktree list runs");
        let list_text = String::from_utf8_lossy(&list.stdout);
        assert_eq!(
            list_text.matches("worktree ").count(),
            N + 1,
            "git itself must report N worktrees plus the primary checkout, got:\n{list_text}"
        );

        // Every assignment shares the identical base commit (a common base commit per R-SA-061).
        let base_commits: std::collections::HashSet<_> =
            plan.assignments.iter().map(|a| a.base_commit.clone()).collect();
        assert_eq!(
            base_commits.len(),
            1,
            "every worktree in the group must be cut from the SAME base commit"
        );
    }

    // ---- R-SA-065: best-effort cleanup removes worktrees even when one item's cleanup fails ----

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cleanup_removes_worktrees_even_when_one_items_cleanup_fails() {
        let repo = make_real_git_repo();
        let worktree_base = tempfile::tempdir().expect("worktree base dir");
        let config = default_config("group-cleanup", worktree_base.path());

        const N: usize = 3;
        let overrides: Vec<Option<&Path>> = vec![None; N];
        let plan = setup_worktree_group(repo.path(), &overrides, &config)
            .await
            .expect("setup succeeds");
        assert_eq!(plan.task_count(), N);

        // Sabotage exactly ONE worktree's removal ahead of time via `git worktree lock` — git
        // refuses to `remove` (even with a single `--force`) a LOCKED worktree, requiring a
        // double `-f -f` this crate's `remove_one_worktree` deliberately does not pass (locking
        // is a real, git-native way to guard a worktree against accidental removal, which is
        // exactly the kind of genuine failure `remove_one_worktree` must surface rather than
        // silently paper over) — while the other two worktrees remain perfectly removable. (An
        // earlier version of this test tried deleting the worktree directory out from under git
        // instead, but modern git's `worktree remove --force` tolerates an already-missing
        // directory and succeeds anyway, so that approach never actually exercised the failure
        // path this test needs — `lock` is the reliable, git-native way to force a real
        // `git worktree remove` failure.)
        let sabotaged_index = 1usize;
        let sabotaged_path = plan.assignments[sabotaged_index].path.clone();
        let lock_status = StdCommand::new("git")
            .current_dir(repo.path())
            .args([
                "worktree",
                "lock",
                "--reason",
                "sabotage-for-test",
                sabotaged_path.to_string_lossy().as_ref(),
            ])
            .status()
            .expect("git worktree lock runs");
        assert!(lock_status.success(), "sabotage: locking one worktree must succeed");

        let failures = cleanup_worktrees(repo.path(), &plan.assignments).await;

        // The sabotaged (locked) worktree must still genuinely exist — its removal really did
        // fail, this is not merely a reported-but-not-real failure.
        assert!(
            sabotaged_path.exists(),
            "the locked worktree's removal must have genuinely failed, leaving it in place"
        );

        // The two non-sabotaged worktrees must have been removed regardless of the sabotaged
        // one's outcome — proving cleanup is genuinely per-item best-effort, not all-or-nothing.
        for (index, assignment) in plan.assignments.iter().enumerate() {
            if index == sabotaged_index {
                continue;
            }
            assert!(
                !assignment.path.exists(),
                "non-sabotaged worktree at {} must have been removed by cleanup",
                assignment.path.display()
            );
        }

        // `git worktree list` must reflect that the non-sabotaged worktrees are gone at the git
        // level too (not merely that their directories vanished).
        let list = StdCommand::new("git")
            .current_dir(repo.path())
            .args(["worktree", "list", "--porcelain"])
            .output()
            .expect("git worktree list runs");
        let list_text = String::from_utf8_lossy(&list.stdout);
        for (index, assignment) in plan.assignments.iter().enumerate() {
            if index == sabotaged_index {
                continue;
            }
            assert!(
                !list_text.contains(assignment.path.to_string_lossy().as_ref()),
                "git itself must no longer list the cleaned-up worktree at {}, got:\n{list_text}",
                assignment.path.display()
            );
        }

        // And cleanup did not silently swallow the one real failure — it is reported back, tagged
        // with the offending index, so a caller can log/surface it.
        assert!(
            !failures.is_empty(),
            "the sabotaged worktree's removal failure must be reported, not silently dropped"
        );
        assert!(
            failures
                .iter()
                .any(|(index, _)| *index == u32::try_from(sabotaged_index).unwrap_or(u32::MAX)),
            "the reported failure must be tagged with the sabotaged worktree's own index"
        );
    }

    // ---- R-SA-063: setup hook JSON stdin/stdout contract + bounded timeout ----

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_successful_hook_receives_the_documented_request_shape_and_its_ok_response_is_applied() {
        let repo = make_real_git_repo();
        let worktree_base = tempfile::tempdir().expect("worktree base dir");

        // A real `sh` script standing in for the hook: read stdin (discard, but implicitly prove
        // it was written to since a hook that never got the payload would hang here and trip the
        // bounded-drain further down if EOF is never observed on our side), echo a fixed `ok`
        // JSON response.
        let hook = HookSpec {
            command: PathBuf::from("sh"),
            args: vec![
                "-c".to_string(),
                "cat > /dev/null; printf '{\"ok\":true}'".to_string(),
            ],
        };
        let config = WorktreeGroupConfig {
            group_id: "group-hook-ok",
            worktree_base_dir: worktree_base.path(),
            setup_hook: Some(&hook),
            setup_hook_timeout_ms: Some(5_000),
        };

        let overrides: Vec<Option<&Path>> = vec![None, None];
        let plan = setup_worktree_group(repo.path(), &overrides, &config)
            .await
            .expect("an ok:true hook response must let setup succeed");
        assert_eq!(plan.task_count(), 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_hook_reporting_ok_false_fails_the_whole_group_and_cleans_up() {
        let repo = make_real_git_repo();
        let worktree_base = tempfile::tempdir().expect("worktree base dir");

        let hook = HookSpec {
            command: PathBuf::from("sh"),
            args: vec![
                "-c".to_string(),
                "cat > /dev/null; printf '{\"ok\":false,\"error\":\"setup script failed\"}'"
                    .to_string(),
            ],
        };
        let config = WorktreeGroupConfig {
            group_id: "group-hook-fail",
            worktree_base_dir: worktree_base.path(),
            setup_hook: Some(&hook),
            setup_hook_timeout_ms: Some(5_000),
        };

        let overrides: Vec<Option<&Path>> = vec![None, None];
        let err = setup_worktree_group(repo.path(), &overrides, &config)
            .await
            .expect_err("ok:false must fail the whole group");
        let SubagentError::WorktreeSetup(message) = err else {
            panic!("expected WorktreeSetup");
        };
        assert!(message.contains("setup script failed"));

        // Cleanup ran: nothing was left behind under the worktree base dir.
        let remaining: Vec<_> = std::fs::read_dir(worktree_base.path())
            .expect("worktree base dir readable")
            .collect();
        assert!(
            remaining.is_empty(),
            "a rejected hook must leave zero worktrees behind (cleanup ran)"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_hook_that_exceeds_its_timeout_fails_the_group_within_the_bound() {
        let repo = make_real_git_repo();
        let worktree_base = tempfile::tempdir().expect("worktree base dir");

        // A real, slow hook: sleeps far longer than the configured timeout.
        let hook = HookSpec {
            command: PathBuf::from("sh"),
            args: vec!["-c".to_string(), "sleep 30".to_string()],
        };
        let config = WorktreeGroupConfig {
            group_id: "group-hook-timeout",
            worktree_base_dir: worktree_base.path(),
            setup_hook: Some(&hook),
            setup_hook_timeout_ms: Some(200),
        };

        let overrides: Vec<Option<&Path>> = vec![None];
        let started = tokio::time::Instant::now();
        let err = setup_worktree_group(repo.path(), &overrides, &config)
            .await
            .expect_err("a hook that outlives its timeout must fail the group");
        assert!(matches!(err, SubagentError::WorktreeSetup(_)));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the timeout must be genuinely bounded, not fall through to the hook's own 30s sleep, \
             got {:?}",
            started.elapsed()
        );
    }

    // ---- R-SA-064: synthetic-path safety rail ----

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_hook_declaring_a_relative_untracked_synthetic_path_is_accepted() {
        let repo = make_real_git_repo();
        let worktree_base = tempfile::tempdir().expect("worktree base dir");

        let hook = HookSpec {
            command: PathBuf::from("sh"),
            args: vec![
                "-c".to_string(),
                "cat > /dev/null; printf '{\"ok\":true,\"synthetic_paths\":[{\"worktree_index\":0,\"path\":\"generated/lock.json\"}]}'"
                    .to_string(),
            ],
        };
        let config = WorktreeGroupConfig {
            group_id: "group-synthetic-ok",
            worktree_base_dir: worktree_base.path(),
            setup_hook: Some(&hook),
            setup_hook_timeout_ms: Some(5_000),
        };

        let overrides: Vec<Option<&Path>> = vec![None];
        let plan = setup_worktree_group(repo.path(), &overrides, &config)
            .await
            .expect("an untracked, relative synthetic path must be accepted");
        assert_eq!(
            plan.assignments[0].synthetic_paths,
            vec![PathBuf::from("generated/lock.json")]
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_hook_declaring_an_absolute_synthetic_path_fails_setup() {
        let repo = make_real_git_repo();
        let worktree_base = tempfile::tempdir().expect("worktree base dir");

        let hook = HookSpec {
            command: PathBuf::from("sh"),
            args: vec![
                "-c".to_string(),
                "cat > /dev/null; printf '{\"ok\":true,\"synthetic_paths\":[{\"worktree_index\":0,\"path\":\"/etc/passwd\"}]}'"
                    .to_string(),
            ],
        };
        let config = WorktreeGroupConfig {
            group_id: "group-synthetic-absolute",
            worktree_base_dir: worktree_base.path(),
            setup_hook: Some(&hook),
            setup_hook_timeout_ms: Some(5_000),
        };

        let overrides: Vec<Option<&Path>> = vec![None];
        let err = setup_worktree_group(repo.path(), &overrides, &config)
            .await
            .expect_err("an absolute synthetic path must fail setup");
        assert!(matches!(err, SubagentError::WorktreeSetup(_)));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_hook_marking_a_tracked_file_as_synthetic_fails_setup_rather_than_hiding_it() {
        let repo = make_real_git_repo();
        let worktree_base = tempfile::tempdir().expect("worktree base dir");

        // README.md is tracked (committed in `make_real_git_repo`) — declaring IT as synthetic
        // must be rejected outright, per R-SA-064's exact text.
        let hook = HookSpec {
            command: PathBuf::from("sh"),
            args: vec![
                "-c".to_string(),
                "cat > /dev/null; printf '{\"ok\":true,\"synthetic_paths\":[{\"worktree_index\":0,\"path\":\"README.md\"}]}'"
                    .to_string(),
            ],
        };
        let config = WorktreeGroupConfig {
            group_id: "group-synthetic-tracked",
            worktree_base_dir: worktree_base.path(),
            setup_hook: Some(&hook),
            setup_hook_timeout_ms: Some(5_000),
        };

        let overrides: Vec<Option<&Path>> = vec![None];
        let err = setup_worktree_group(repo.path(), &overrides, &config)
            .await
            .expect_err("marking a TRACKED file as synthetic must fail setup, not hide it");
        let SubagentError::WorktreeSetup(message) = err else {
            panic!("expected WorktreeSetup");
        };
        assert!(message.contains("README.md"));

        // And cleanup ran — the rejected group leaves no worktrees behind.
        let remaining: Vec<_> = std::fs::read_dir(worktree_base.path())
            .expect("worktree base dir readable")
            .collect();
        assert!(remaining.is_empty(), "a rejected group must clean up after itself");
    }

    // ---- is_tracked_file: direct unit coverage of the safety-rail primitive ----

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn is_tracked_file_distinguishes_tracked_from_untracked() {
        let repo = make_real_git_repo();
        assert!(
            is_tracked_file(repo.path(), Path::new("README.md"))
                .await
                .expect("check runs"),
            "README.md was committed by the fixture and must be reported tracked"
        );
        assert!(
            !is_tracked_file(repo.path(), Path::new("never-existed.txt"))
                .await
                .expect("check runs"),
            "a path that was never created/tracked must be reported untracked"
        );
    }
}
