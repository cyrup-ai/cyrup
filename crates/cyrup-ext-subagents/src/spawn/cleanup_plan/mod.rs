//! The two-phase worktree cleanup PLAN — pi `src/runs/shared/worktree-cleanup-plan.ts` @`v0.68.0`
//! (869 lines, read in full).
//!
//! A `subagent({tasks:[…], worktree:true})` fan-out allocates N isolated git worktrees and
//! publishes a [manifest](crate::handoff) recording what each lane claimed and whether its
//! worktree is still on disk. This module is the half that decides **which of those worktrees it
//! is safe to remove** — by cross-checking `git worktree list --porcelain` against that manifest,
//! and refusing to propose a removal for anything it cannot prove.
//!
//! # It is PLAN-ONLY, and that is upstream's design, not an unfinished port
//!
//! Upstream has no apply phase at `v0.68.0`. `subagent-executor.ts:6217-6222` refuses
//! `mode != "plan"` and refuses `planId` outright, `schemas.ts:300` describes `planId` as
//! *"Reserved; cleanup is plan-only."*, and the module has exactly two consumers in the whole
//! upstream tree (the import at `:154` and the single call at `:6224`). There is no plan reader
//! and no TTL enforcement anywhere. [`WorktreeCleanupPlan::expires_at`], `content_hash` and
//! `preconditions` are written completely because they are the **forward contract** that makes a
//! later apply phase safe; nothing reads them yet, and implementing an executor here would be
//! inventing behaviour rather than porting it.
//!
//! The invariant "a stale plan must never be executed" is therefore discharged STRUCTURALLY: the
//! dispatch boundary refuses `mode='apply'` and refuses `planId`, with upstream's own sentences.
//! Those refusals are part of the deliverable precisely because they ARE the guard.
//!
//! # Why it lives beside `spawn/worktree.rs`
//!
//! It is the only consumer of that file's private git helpers (`run_git`, `run_git_env`,
//! `GitResult`, `resolve_worktree_base_dir_path`, `lexical_normalize`,
//! `normalize_comparable_cwd`), all of which are `pub(crate)` for it. Nothing outside the tool
//! dispatch imports this module.
//!
//! # What it must never do
//!
//! Building a plan mutates nothing but the plan file. `crate::spawn::worktree::cleanup_worktrees`
//! — the force-removing primitive — is deliberately NOT imported here, and neither
//! `git worktree remove`, `git branch -D` nor `git worktree prune` appears anywhere in the
//! module. [`WorktreeCleanupPlan::prune_candidates`] is a report (pi `:862`). The one
//! write-shaped git sequence, the patch re-capture in `classify::is_patch_captured`, stages into
//! a temporary `GIT_INDEX_FILE` and never the worktree's own index.

mod classify;
mod git;
mod gitwt;
mod metadata;
pub mod model;
mod paths;
mod render;
mod verdict;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use classify::ClassifyContext;
use model::{
    CleanupDecision, CleanupPlanEntry, CleanupState, ContentHash, CreatedCleanupPlan,
    ForegroundRunOwnership, MAX_PLAN_ENTRIES, PlanId, WORKTREE_CLEANUP_PLAN_TTL_MS,
    WORKTREE_CLEANUP_PLAN_VERSION, WorktreeCleanupPlan,
};
use paths::comparable_path;

pub use model::{CleanupPlanError, worktree_cleanup_plan_path};
pub use render::format_worktree_cleanup_plan;

/// The injected proof that a FOREGROUND owning run has finished — pi's
/// `foregroundRunOwnership?: (runId) => ForegroundRunOwnership` (`:81`).
///
/// Deliberately **not** `Option`. Upstream's call site is `foregroundRunOwnership?.(runId) ??
/// "unknown"` (`:497`), so an absent probe and an unknown answer are indistinguishable — which
/// means the `Option` carries no information and only adds a way to be wrong. Making it required
/// removes the possibility that "nobody supplied a probe" silently reads as "terminal".
///
/// `+ Sync` because the plan builder is `async` and the tool dispatch's future must stay `Send`:
/// the probe is held across `await` points while git subprocesses run.
pub type ForegroundOwnershipProbe<'a> = &'a (dyn Fn(&str) -> ForegroundRunOwnership + Sync);

/// pi `BuildWorktreeCleanupPlanInput` (`:72-82`).
pub struct BuildCleanupPlanInput<'a> {
    /// The repository to plan for. **Required** — pi's `:743` "requires a repository path" guard
    /// is this type.
    pub repo: &'a Path,
    /// A manifest the caller named explicitly. Its read failures are warned about; a merely
    /// DISCOVERED file's are not (pi `:429`).
    pub handoff_path: Option<&'a Path>,
    /// pi's `handoffPaths` test-and-migration seam (`:74`).
    pub handoff_paths: &'a [PathBuf],
    /// `subagents.worktreeBaseDir`, when the operator configured one.
    pub worktree_base_dir: Option<&'a str>,
    /// Epoch millis.
    ///
    /// An `i64`, which is what discharges pi's `:746` `Number.isFinite(now)` throw: an integer
    /// epoch cannot be NaN, so the guard is the type and [`CleanupPlanError`] carries no variant
    /// for it.
    pub now: i64,
    /// An explicit plan id, or [`PlanId::generate`] when absent.
    pub plan_id: Option<PlanId>,
    /// See [`ForegroundOwnershipProbe`].
    pub foreground_ownership: ForegroundOwnershipProbe<'a>,
}

/// pi `resolveRepoRoot` (`:194-198`).
async fn resolve_repo_root(repo: &Path) -> Result<PathBuf, CleanupPlanError> {
    let requested = std::path::absolute(repo).unwrap_or_else(|_| repo.to_path_buf());
    let toplevel = git::require_success(
        git::run(&requested, &["rev-parse", "--show-toplevel"]).await,
        &format!("git -C {} rev-parse --show-toplevel", requested.display()),
    )?;
    Ok(paths::resolve_existing_path(Path::new(toplevel.trim())))
}

/// pi `cleanupContainmentInvalid` (`:229-234`) — the base directory must not BE the repository,
/// must not be inside it, and must not be inside the agent's extensions directory.
fn containment_invalid(repo_root: &Path, base_dir: &Path) -> bool {
    let extensions_dir = crate::paths::agent_dir().join("extensions");
    paths::same_path(base_dir, repo_root)
        || paths::path_inside(repo_root, base_dir, true)
        || paths::path_inside(&extensions_dir, base_dir, false)
}

/// pi `resolveCleanupBaseDir` (`:213-227`), with the one divergence this area could not avoid.
///
/// [CYRUP-DELTA] upstream REPRODUCES its own create-side layout here
/// (`<dirname(repoRoot)>/worktrees/<basename(repoRoot)>`, verified against `worktree.ts:648-651`
/// and `buildNativeWorktreePath:684-686`), which is what makes the strict-child containment check
/// at `:628` pass for a pi-created worktree. **cyrup's create side is different**:
/// `spawn::worktree::resolve_worktree_base_dir_path` resolves
/// `configured -> $CYRUP_SUBAGENTS_WORKTREE_DIR -> std::env::temp_dir()` with no `worktrees/`
/// rung and no per-repository level, and the leaf lands directly at
/// `<base>/cyrup-worktree-<runId>-<index>`. Transliterating pi's resolver would check containment
/// against a directory this build never writes to, making every entry `Ineligible/Keep` — a
/// cleanup verb that can never propose anything. So the plan calls **the same function the create
/// side calls**, which is the actual invariant pi's version encodes.
///
/// The tightening that goes with it lives in [`classify`]: see
/// [`verdict::IneligibleReason::NotAManagedWorktreeName`].
fn resolve_cleanup_base_dir(
    repo_root: &Path,
    configured: Option<&str>,
) -> Result<PathBuf, CleanupPlanError> {
    crate::spawn::worktree::resolve_worktree_base_dir_path(configured, repo_root)
        .map_err(|_| CleanupPlanError::EmptyBaseDir)
}

/// Build a cleanup plan without persisting it — pi `buildWorktreeCleanupPlan` (`:742-819`).
///
/// # Errors
///
/// [`CleanupPlanError`] only for the six conditions that abort the WHOLE plan (a missing repo, an
/// unusable base directory, a bad plan id, a failing `rev-parse`/`worktree list`). Everything
/// else is a per-entry verdict or a warning: a cleanup planner whose job is to be cautious must
/// degrade to "keep, here is why", not to an error.
pub async fn build_worktree_cleanup_plan(
    input: BuildCleanupPlanInput<'_>,
) -> Result<WorktreeCleanupPlan, CleanupPlanError> {
    if input.repo.as_os_str().is_empty() {
        return Err(CleanupPlanError::MissingRepo);
    }
    let repo_root = resolve_repo_root(input.repo).await?;
    let base_dir = resolve_cleanup_base_dir(&repo_root, input.worktree_base_dir)?;
    let containment_invalid = containment_invalid(&repo_root, &base_dir);
    let git_records = gitwt::list_git_worktrees(&repo_root).await?;
    let target_head = git::require_success(
        git::run(&repo_root, &["rev-parse", "HEAD"]).await,
        &format!("git -C {} rev-parse HEAD", repo_root.display()),
    )?;
    let root_path = comparable_path(&repo_root);

    let metadata =
        metadata::load_metadata(&repo_root, input.handoff_path, input.handoff_paths).await;
    let mut warnings: BTreeSet<String> = metadata.warnings;

    let ctx = ClassifyContext {
        repo_root: &repo_root,
        base_dir: &base_dir,
        containment_invalid,
        target_head: &target_head,
        root_path: &root_path,
        all_git_records: &git_records,
        now: input.now,
        foreground_ownership: input.foreground_ownership,
    };

    // The repository's own worktree is never a candidate (pi `:761`).
    let linked: Vec<&gitwt::GitWorktreeRecord> = git_records
        .iter()
        .filter(|record| !paths::same_path(&record.path, &repo_root))
        .collect();

    let mut entries: Vec<CleanupPlanEntry> = Vec::new();
    let mut matched: Vec<usize> = Vec::new();
    for git_record in &linked {
        // pi `:766-768`: join by comparable path, falling back to a branch+path match. Anything
        // other than EXACTLY ONE matching metadata record is `Unknown` — an ambiguous claim on a
        // worktree is not a claim.
        let key = comparable_path(&git_record.path);
        let candidates: Vec<usize> = {
            let by_path: Vec<usize> = metadata
                .records
                .iter()
                .enumerate()
                .filter(|(_, record)| comparable_path(&record.worktree_path()) == key)
                .map(|(index, _)| index)
                .collect();
            if by_path.is_empty() && git_record.branch.is_some() {
                metadata
                    .records
                    .iter()
                    .enumerate()
                    .filter(|(_, record)| {
                        Some(record.task.branch.as_str()) == git_record.branch.as_deref()
                            && paths::same_path(&record.worktree_path(), &git_record.path)
                    })
                    .map(|(index, _)| index)
                    .collect()
            } else {
                by_path
            }
        };
        if let [only] = candidates[..] {
            matched.push(only);
            if let Some(record) = metadata.records.get(only) {
                entries.push(classify::managed_entry(&ctx, git_record, record).await);
            }
        } else {
            entries.push(classify::unknown_git_entry(git_record, &target_head));
        }
    }

    // pi `:785-791`: metadata rows git never listed.
    let git_keys: Vec<PathBuf> = linked
        .iter()
        .map(|record| comparable_path(&record.path))
        .collect();
    for (index, record) in metadata.records.iter().enumerate() {
        if matched.contains(&index) {
            continue;
        }
        let key = comparable_path(&record.worktree_path());
        if git_keys.contains(&key) {
            continue;
        }
        // pi keys this loop by PATH and skips any path claimed by more than one record, for the
        // same reason the git-side join does.
        if metadata
            .records
            .iter()
            .filter(|other| comparable_path(&other.worktree_path()) == key)
            .count()
            != 1
        {
            continue;
        }
        if let Some(entry) = classify::missing_metadata_entry(record, &target_head) {
            entries.push(entry);
        }
    }

    // pi `stableEntrySort` (`:738-740`).
    entries.sort_by(|left, right| {
        comparable_path(&left.path)
            .cmp(&comparable_path(&right.path))
            .then_with(|| left.branch.cmp(&right.branch))
    });
    if entries.len() > MAX_PLAN_ENTRIES {
        warnings.insert(format!(
            "cleanup plan entry count capped at {MAX_PLAN_ENTRIES}; remaining worktrees are not evaluated"
        ));
        entries.truncate(MAX_PLAN_ENTRIES);
    }

    // pi `:797-800`: a stale, undecided entry that git itself calls prunable. REPORTED ONLY.
    let mut prune_candidates: Vec<PathBuf> = entries
        .iter()
        .filter(|entry| {
            entry.decision == CleanupDecision::Unknown && entry.state == CleanupState::Stale
        })
        .filter(|entry| {
            git_records.iter().any(|record| {
                record.prunable.is_some()
                    && comparable_path(&record.path) == comparable_path(&entry.path)
            })
        })
        .map(|entry| entry.path.clone())
        .collect();
    prune_candidates.sort_by_key(|path| comparable_path(path));

    let warnings = (!warnings.is_empty()).then(|| warnings.into_iter().collect::<Vec<_>>());
    let content_hash = content_hash(
        &repo_root,
        &base_dir,
        &metadata.paths,
        &entries,
        &prune_candidates,
        warnings.as_deref(),
    );
    let plan_id = input.plan_id.unwrap_or_else(PlanId::generate);

    Ok(WorktreeCleanupPlan {
        version: WORKTREE_CLEANUP_PLAN_VERSION,
        plan_id,
        repo_root,
        created_at: input.now,
        expires_at: input.now.saturating_add(WORKTREE_CLEANUP_PLAN_TTL_MS),
        base_dirs: vec![base_dir],
        metadata_paths: metadata.paths,
        entries,
        prune_candidates,
        warnings,
        content_hash,
    })
}

/// pi `contentPayload` (`:726-736`) + `sha256` (`:234-236`).
///
/// A recomputability guarantee for a future apply phase — "these are the same facts I planned
/// against" — and deliberately **not** a byte-parity claim against pi. The key order matches
/// upstream's payload because the workspace's `serde_json` carries `preserve_order`, so a reader
/// that recomputes it from the plan's own fields gets the same bytes.
fn content_hash(
    repo_root: &Path,
    base_dir: &Path,
    metadata_paths: &[PathBuf],
    entries: &[CleanupPlanEntry],
    prune_candidates: &[PathBuf],
    warnings: Option<&[String]>,
) -> ContentHash {
    let mut payload = serde_json::Map::new();
    payload.insert(
        "version".into(),
        serde_json::json!(WORKTREE_CLEANUP_PLAN_VERSION),
    );
    payload.insert("repoRoot".into(), serde_json::json!(repo_root));
    payload.insert("baseDirs".into(), serde_json::json!([base_dir]));
    payload.insert("metadataPaths".into(), serde_json::json!(metadata_paths));
    payload.insert(
        "entries".into(),
        serde_json::to_value(entries).unwrap_or(serde_json::Value::Null),
    );
    payload.insert(
        "pruneCandidates".into(),
        serde_json::json!(prune_candidates),
    );
    if let Some(warnings) = warnings {
        payload.insert("warnings".into(), serde_json::json!(warnings));
    }
    let bytes = serde_json::to_vec(&serde_json::Value::Object(payload)).unwrap_or_default();
    ContentHash::of(&bytes)
}

/// Build a plan and PERSIST it — pi `createWorktreeCleanupPlan` (`:825-830`).
///
/// This write is the only mutation the whole verb performs.
///
/// # Errors
///
/// As [`build_worktree_cleanup_plan`], plus [`CleanupPlanError::Write`].
pub async fn create_worktree_cleanup_plan(
    input: BuildCleanupPlanInput<'_>,
) -> Result<CreatedCleanupPlan, CleanupPlanError> {
    let plan = build_worktree_cleanup_plan(input).await?;
    let plan_path = worktree_cleanup_plan_path(&plan.repo_root, &plan.plan_id);
    crate::background::atomic::write_atomic_json_creating_parent(&plan_path, &plan)
        .await
        .map_err(|source| CleanupPlanError::Write {
            path: plan_path.clone(),
            source,
        })?;
    Ok(CreatedCleanupPlan { plan, plan_path })
}
