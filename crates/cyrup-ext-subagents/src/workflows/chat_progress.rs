//! The workflow live-card projection and row builder — ports pi `workflows/chat-progress.ts`
//! (152 LOC @ `df26ebc8`): the mode vocabulary (`:7-9`), the git repository identity (`:29-65`),
//! `resolveWorkflowChatProgress` (`:75-96`) and `buildWorkflowChatProgressRows` (`:113-152`).
//!
//! The one upstream import this module does not port is `workflowPreflightLaneForRuntimeKey`
//! (`workflow-preflight.ts:154-168`, SCOPE_3e); the row builder takes that lookup as a parameter
//! so the rows' `preflight` field is populated the day the preflight module lands and this module
//! ships complete today.

use std::path::{Path, PathBuf};

use super::key::WorkflowKey;
use super::types::{
    WorkflowPreflightLane, WorkflowScriptOperation, WorkflowScriptTraceEntry,
    WorkflowScriptTraceState,
};

/// The requestable chat-progress modes — pi `WORKFLOW_CHAT_PROGRESS_MODES` (`chat-progress.ts:7`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkflowChatProgressMode {
    /// Resolve to `live-card` when same-repo and foreground, else `off`.
    Auto,
    /// No live card.
    Off,
    /// An inline live card in the parent chat.
    LiveCard,
}

impl WorkflowChatProgressMode {
    /// The three mode words, in upstream's declaration order — the error message is built from
    /// this list (`chat-progress.ts:70`).
    const WORDS: [(&'static str, Self); 3] = [
        ("auto", Self::Auto),
        ("off", Self::Off),
        ("live-card", Self::LiveCard),
    ];
}

/// A chat-progress mode with `auto` already resolved away — pi
/// `ResolvedWorkflowChatProgressMode = Exclude<WorkflowChatProgressMode, "auto">`
/// (`chat-progress.ts:9`), a type-level fact rather than a convention.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResolvedWorkflowChatProgressMode {
    /// No live card.
    Off,
    /// An inline live card in the parent chat.
    LiveCard,
}

/// pi `WorkflowChatProgressProjection.repoRelation` (`chat-progress.ts:18`) — two values, so an
/// enum, not a bool named `same_repo` (SCOPE_3 §A.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RepoRelation {
    /// Parent and workflow cwd resolve to one repository.
    Same,
    /// Different repositories, or either side is not a repository.
    Other,
}

/// pi `WorkflowChatProgressProjection` (`chat-progress.ts:16-20`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowChatProgressProjection {
    /// The resolved mode.
    pub mode: ResolvedWorkflowChatProgressMode,
    /// How the workflow cwd relates to the parent's repository.
    pub repo_relation: RepoRelation,
    /// The workflow repository's basename, when the workflow cwd is a repository (`:85`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_label: Option<String>,
}

/// A git repository's identity for same-repo comparison — pi `GitRepositoryIdentity`
/// (`chat-progress.ts:11-14`).
///
/// `common_dir` is the load-bearing half and the reason `root` alone will not do: two linked
/// WORKTREES of one repository have different roots and the SAME common dir, so comparing only
/// roots would call them different repositories and silently disable the live card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitRepositoryIdentity {
    /// The worktree root (`git rev-parse --show-toplevel`), canonicalized.
    pub root: PathBuf,
    /// The shared `.git` common directory (`git rev-parse --git-common-dir`), canonicalized.
    pub common_dir: PathBuf,
}

/// pi `realPath` (`chat-progress.ts:36-42`): `fs.realpathSync.native` with a lexical
/// `path.resolve` fallback, so a symlinked checkout still compares equal and a non-existent path
/// still becomes absolute.
fn real_path(value: &Path) -> PathBuf {
    std::fs::canonicalize(value)
        .or_else(|_| std::path::absolute(value))
        .unwrap_or_else(|_| value.to_path_buf())
}

/// pi `resolveGitRepositoryIdentity` (`chat-progress.ts:44-56`).
///
/// Upstream needs three `git rev-parse` subprocesses plus the relative-path reconstruction at
/// `:49-51` that only exists because `--git-common-dir` may answer relatively. `gix` exposes both
/// values directly and absolutely, so that whole dance is structurally absent:
///
/// | pi | cyrup |
/// |---|---|
/// | `rev-parse --is-inside-work-tree` != `"true"` ⇒ undefined | `Repository::workdir()` is `None` (bare) ⇒ `None` |
/// | `rev-parse --show-toplevel` | `Repository::workdir()` |
/// | `rev-parse --git-common-dir` + `:49-51` | `Repository::common_dir()` |
///
/// A non-repository `cwd` is the ordinary case, not an error — any discovery failure is `None`,
/// exactly as `exec/mutation_evidence/repo.rs` already documents for the same `gix::discover`
/// call. In-process `gix` rather than a `git` subprocess for that module's stated reason, too:
/// shelling out imports the user's `diff.external`/`core.pager`/fsmonitor configuration into a
/// display decision.
#[must_use]
pub fn resolve_git_repository_identity(cwd: &Path) -> Option<GitRepositoryIdentity> {
    let repo = gix::discover(cwd).ok()?;
    let root = repo.workdir()?.to_path_buf();
    let common_dir = repo.common_dir().to_path_buf();
    Some(GitRepositoryIdentity {
        root: real_path(&root),
        common_dir: real_path(&common_dir),
    })
}

/// pi `isSameGitRepositoryIdentity` (`chat-progress.ts:58-61`): `common_dir` equal **or** `root`
/// equal — and `None` on either side is `false`, never "assume same".
#[must_use]
pub fn is_same_git_repository_identity(
    left: Option<&GitRepositoryIdentity>,
    right: Option<&GitRepositoryIdentity>,
) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left.common_dir == right.common_dir || left.root == right.root,
        (None, _) | (_, None) => false,
    }
}

/// pi `isSameGitRepository` (`chat-progress.ts:63-65`).
#[must_use]
pub fn is_same_git_repository(left_cwd: &Path, right_cwd: &Path) -> bool {
    is_same_git_repository_identity(
        resolve_git_repository_identity(left_cwd).as_ref(),
        resolve_git_repository_identity(right_cwd).as_ref(),
    )
}

/// pi `normalizeRequestedMode` (`chat-progress.ts:67-73`): absent ⇒ `auto`; a non-string or
/// unknown string ⇒ the error listing the mode words.
fn normalize_requested_mode(
    value: Option<&serde_json::Value>,
) -> Result<WorkflowChatProgressMode, String> {
    let Some(value) = value else {
        return Ok(WorkflowChatProgressMode::Auto);
    };
    value
        .as_str()
        .and_then(|word| {
            WorkflowChatProgressMode::WORDS
                .iter()
                .find(|(candidate, _)| *candidate == word)
                .map(|(_, mode)| *mode)
        })
        .ok_or_else(|| {
            let words: Vec<&str> = WorkflowChatProgressMode::WORDS
                .iter()
                .map(|(word, _)| *word)
                .collect();
            format!("chatProgress must be one of: {}.", words.join(", "))
        })
}

/// The mode decision — pi `chat-progress.ts:88-94`, as one pure function rather than statement
/// order (SCOPE_3 §A.1): `auto` resolves first (`live-card` iff same-repo and foreground, `:90`),
/// and the two refusals apply to the RESOLVED mode — so an `Auto` that resolved to `LiveCard`
/// already proved `same_repo && !background` and is unreachable-by-construction for both, in a
/// way a reader can see.
fn resolve_mode(
    requested: WorkflowChatProgressMode,
    same_repo: bool,
    background: bool,
) -> Result<ResolvedWorkflowChatProgressMode, String> {
    let mode = match requested {
        WorkflowChatProgressMode::Auto => {
            if same_repo && !background {
                ResolvedWorkflowChatProgressMode::LiveCard
            } else {
                ResolvedWorkflowChatProgressMode::Off
            }
        }
        WorkflowChatProgressMode::Off => ResolvedWorkflowChatProgressMode::Off,
        WorkflowChatProgressMode::LiveCard => ResolvedWorkflowChatProgressMode::LiveCard,
    };
    if mode == ResolvedWorkflowChatProgressMode::LiveCard && !same_repo {
        return Err(
            "chatProgress: 'live-card' is only available for workflowScript runs in the same Git repository."
                .to_string(),
        );
    }
    if mode == ResolvedWorkflowChatProgressMode::LiveCard && background {
        return Err(
            "chatProgress: 'live-card' is unavailable for async workflowScript. Async workflows have no inline live card; omit chatProgress or use auto/off. Use async:false only when the parent must block."
                .to_string(),
        );
    }
    Ok(mode)
}

/// pi `ResolveWorkflowChatProgressInput` (`chat-progress.ts:22-27`).
#[derive(Clone, Copy, Debug)]
pub struct ResolveWorkflowChatProgressInput<'a> {
    /// The raw `chatProgress` request off the tool params (`unknown` upstream): `None` for
    /// absent.
    pub requested: Option<&'a serde_json::Value>,
    /// The parent session's cwd.
    pub parent_cwd: &'a Path,
    /// The workflow's cwd.
    pub workflow_cwd: &'a Path,
    /// Whether the workflow runs async (background).
    pub background: bool,
}

/// pi `resolveWorkflowChatProgress` (`chat-progress.ts:75-96`).
///
/// `Result` rather than `Result<Option<_>, _>`: upstream returns exactly one of
/// `projection`/`error` on every path (`:77`, `:93`, `:94`, `:95`), so an `Ok(None)` state cannot
/// occur and is not representable.
///
/// # Errors
///
/// The `normalizeRequestedMode` message for an unknown request, or one of the two verbatim
/// `live-card` refusals (`:93`/`:94`) — see [`WorkflowChatProgressProjection`] for the success
/// shape.
pub fn resolve_workflow_chat_progress(
    input: ResolveWorkflowChatProgressInput<'_>,
) -> Result<WorkflowChatProgressProjection, String> {
    let requested = normalize_requested_mode(input.requested)?;
    let parent_identity = resolve_git_repository_identity(input.parent_cwd);
    let workflow_identity = resolve_git_repository_identity(input.workflow_cwd);
    let same_repo =
        is_same_git_repository_identity(parent_identity.as_ref(), workflow_identity.as_ref());
    let repo_label = workflow_identity.as_ref().and_then(|identity| {
        identity
            .root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
    });
    let repo_relation = if same_repo {
        RepoRelation::Same
    } else {
        RepoRelation::Other
    };
    let mode = resolve_mode(requested, same_repo, input.background)?;
    Ok(WorkflowChatProgressProjection {
        mode,
        repo_relation,
        repo_label,
    })
}

/// One live-card row's state word — pi `WorkflowChatProgressRow["state"]`
/// (`chat-progress.ts:100`), SIX variants of which the row builder produces five.
///
/// Note the vocabulary: trace `"completed"` maps to row **`complete`** (`:130-131`) — the
/// opposite spelling from the child-summary's `completed`. Two adjacent enums, two spellings of
/// one idea; they are distinct Rust types and are never `as_str()`-bridged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkflowChatProgressRowState {
    // SCOPE_3e: populated by the preflight path — a lane known from `WorkflowPreflight.lanes`
    // that has not yet appeared in the trace. `buildWorkflowChatProgressRows` never emits it
    // (every row it creates starts `running`, `:128`).
    /// Known from preflight, not yet started.
    Planned,
    /// Started, not yet settled.
    Running,
    /// Settled successfully.
    Complete,
    /// Settled with a failure.
    Failed,
    /// Detached into a background run.
    Detached,
    /// Stopped by an explicit stop request.
    Stopped,
}

/// One row of the workflow live card — pi `WorkflowChatProgressRow` (`chat-progress.ts:98-107`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowChatProgressRow {
    /// The row's workflow key.
    pub key: WorkflowKey,
    /// The row's state word.
    pub state: WorkflowChatProgressRowState,
    /// Display label — assigned only when a trace entry supplies a non-blank one (`:139-142`),
    /// unlike the three clearable fields below; that asymmetry is upstream's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Display phase — same assignment rule as `label`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    /// The child's run id — CLEARED when a later entry omits it (`:143-144`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Elapsed milliseconds — cleared like `run_id` (`:145-146`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// The child's error text — cleared like `run_id` (`:147-148`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The advisory preflight lane for this key — set ONCE, first lane wins (`:129`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preflight: Option<WorkflowPreflightLane>,
}

/// pi `cleanLabel` (`chat-progress.ts:109-111`): trimmed, and only when non-blank.
fn clean_label(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|trimmed| !trimmed.is_empty())
        .map(str::to_string)
}

/// The preflight lane lookup the row builder needs — `workflowPreflightLaneForRuntimeKey`
/// (`workflow-preflight.ts:154-168`, SCOPE_3e), taken as a parameter so this module ships
/// complete before the preflight module lands (SCOPE_3d §0.5). Arguments are the runtime key and
/// the entry's generated-lane provenance key (pi's one-element `preferredKeys`).
pub type WorkflowPreflightLaneLookup<'a> =
    &'a dyn Fn(&WorkflowKey, Option<&WorkflowKey>) -> Option<WorkflowPreflightLane>;

/// pi `buildWorkflowChatProgressRows` (`chat-progress.ts:113-152`), over the script trace, in
/// **insertion order** (`[...rows.values()]`, `:151` — the map is a `Vec` here for the same
/// reason as the child summary's, SCOPE_3d §0.16).
///
/// `lane_for_key: None` is "no preflight supplied" — every row ships with `preflight` absent,
/// upstream's own behaviour when `preflight` is `undefined`.
///
/// [CYRUP-DELTA] Upstream's builder does not re-validate `entry.key` (the workflow executor
/// already did, at every `runs.*` boundary); this port's rows carry a typed [`WorkflowKey`], so
/// an entry whose key fails the grammar — impossible from the executor, by the same validation —
/// is skipped rather than rowed.
#[must_use]
pub fn build_workflow_chat_progress_rows(
    trace: &[WorkflowScriptTraceEntry],
    lane_for_key: Option<WorkflowPreflightLaneLookup<'_>>,
) -> Vec<WorkflowChatProgressRow> {
    let mut rows: Vec<(WorkflowKey, WorkflowChatProgressRow)> = Vec::new();
    for entry in trace {
        if entry.operation != WorkflowScriptOperation::Run {
            continue;
        }
        let Ok(key) = WorkflowKey::parse(&entry.key) else {
            continue;
        };
        // The `"reused"` branch (`:118-126`): update `label`/`phase` on an EXISTING row and move
        // on. It never creates a row and never touches `state` — a reused lane that was never run
        // must not appear as `running`.
        if entry.state == WorkflowScriptTraceState::Reused {
            if let Some((_, row)) = rows.iter_mut().find(|(existing, _)| *existing == key) {
                if let Some(label) = clean_label(entry.label.as_deref()) {
                    row.label = Some(label);
                }
                if let Some(phase) = clean_label(entry.phase.as_deref()) {
                    row.phase = Some(phase);
                }
            }
            continue;
        }
        let generated = entry
            .generated_lane_key
            .as_deref()
            .and_then(|raw| WorkflowKey::parse(raw).ok());
        let lane = lane_for_key.and_then(|lookup| lookup(&key, generated.as_ref()));
        let mut next = rows
            .iter()
            .find(|(existing, _)| *existing == key)
            .map(|(_, row)| row.clone())
            .unwrap_or(WorkflowChatProgressRow {
                key: key.clone(),
                state: WorkflowChatProgressRowState::Running,
                label: None,
                phase: None,
                run_id: None,
                duration_ms: None,
                error: None,
                preflight: None,
            });
        // `preflight` is set once (`:129`): the first lane wins and a later entry cannot
        // overwrite it.
        if next.preflight.is_none() {
            next.preflight = lane;
        }
        next.state = match entry.state {
            WorkflowScriptTraceState::Completed => WorkflowChatProgressRowState::Complete,
            WorkflowScriptTraceState::Failed => WorkflowChatProgressRowState::Failed,
            WorkflowScriptTraceState::Detached => WorkflowChatProgressRowState::Detached,
            WorkflowScriptTraceState::Stopped => WorkflowChatProgressRowState::Stopped,
            // `reused` cannot reach this match (handled above); everything else is running —
            // upstream's own default arm (`:136-138`).
            WorkflowScriptTraceState::Started
            | WorkflowScriptTraceState::Reused
            | WorkflowScriptTraceState::Queued
            | WorkflowScriptTraceState::Delivered
            | WorkflowScriptTraceState::Missed => WorkflowChatProgressRowState::Running,
        };
        // `label`/`phase` assign only when present-and-non-blank (`:139-142`)…
        if let Some(label) = clean_label(entry.label.as_deref()) {
            next.label = Some(label);
        }
        if let Some(phase) = clean_label(entry.phase.as_deref()) {
            next.phase = Some(phase);
        }
        // …while `runId`/`durationMs`/`error` are plain assignments of the whole `Option`
        // (`:143-148`): an entry that OMITS one `delete`s it upstream, and `if let Some` here
        // would leave a stale value on a row whose newer trace entry dropped it — the exact bug
        // the `delete` prevents.
        next.run_id = entry.run_id.clone();
        next.duration_ms = entry.duration_ms;
        next.error = entry.error.clone();
        if let Some(slot) = rows.iter_mut().find(|(existing, _)| *existing == key) {
            slot.1 = next;
        } else {
            rows.push((key, next));
        }
    }
    rows.into_iter().map(|(_, row)| row).collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use serde_json::json;

    use super::super::types::WorkflowScriptOperation;
    use super::*;

    fn entry(
        raw_key: &str,
        state: WorkflowScriptTraceState,
        mutate: impl FnOnce(&mut WorkflowScriptTraceEntry),
    ) -> WorkflowScriptTraceEntry {
        let mut entry = WorkflowScriptTraceEntry {
            operation: WorkflowScriptOperation::Run,
            key: raw_key.to_string(),
            state,
            agent: None,
            run_id: None,
            duration_ms: None,
            phase: None,
            label: None,
            error: None,
            generated_lane_key: None,
            lane: None,
            warning: None,
        };
        mutate(&mut entry);
        entry
    }

    /// `normalizeRequestedMode` + the pure mode decision: absent is auto; auto resolves off for
    /// other-repo or background WITHOUT tripping the refusals; explicit live-card trips them, in
    /// upstream's order and wording.
    #[test]
    fn mode_resolution_matches_upstream() {
        assert_eq!(
            normalize_requested_mode(None),
            Ok(WorkflowChatProgressMode::Auto)
        );
        assert_eq!(
            normalize_requested_mode(Some(&json!("sideways"))),
            Err("chatProgress must be one of: auto, off, live-card.".to_string())
        );
        assert_eq!(
            normalize_requested_mode(Some(&json!(7))),
            Err("chatProgress must be one of: auto, off, live-card.".to_string())
        );
        // Auto: live-card iff same-repo and foreground; never a refusal.
        assert_eq!(
            resolve_mode(WorkflowChatProgressMode::Auto, true, false),
            Ok(ResolvedWorkflowChatProgressMode::LiveCard)
        );
        assert_eq!(
            resolve_mode(WorkflowChatProgressMode::Auto, false, false),
            Ok(ResolvedWorkflowChatProgressMode::Off)
        );
        assert_eq!(
            resolve_mode(WorkflowChatProgressMode::Auto, true, true),
            Ok(ResolvedWorkflowChatProgressMode::Off)
        );
        // Explicit live-card: the same-repo refusal outranks the async one.
        assert_eq!(
            resolve_mode(WorkflowChatProgressMode::LiveCard, false, true),
            Err("chatProgress: 'live-card' is only available for workflowScript runs in the same Git repository.".to_string())
        );
        assert_eq!(
            resolve_mode(WorkflowChatProgressMode::LiveCard, true, true),
            Err("chatProgress: 'live-card' is unavailable for async workflowScript. Async workflows have no inline live card; omit chatProgress or use auto/off. Use async:false only when the parent must block.".to_string())
        );
        assert_eq!(
            resolve_mode(WorkflowChatProgressMode::Off, false, true),
            Ok(ResolvedWorkflowChatProgressMode::Off)
        );
    }

    /// End-to-end over real directories: a repository is `Same` as itself (and a linked worktree
    /// of it, sharing the common dir), and a plain directory is `Other` with no label.
    #[test]
    fn projection_resolves_repo_identity_with_gix() {
        let scratch = tempdir();
        let repo_dir = scratch.join("repo-a");
        std::fs::create_dir_all(repo_dir.join("sub")).expect("mkdir");
        gix::init(&repo_dir).expect("init");
        let plain = scratch.join("plain");
        std::fs::create_dir_all(&plain).expect("mkdir");

        // Same repository (a subdirectory discovers the same root).
        let same = resolve_workflow_chat_progress(ResolveWorkflowChatProgressInput {
            requested: None,
            parent_cwd: &repo_dir,
            workflow_cwd: &repo_dir.join("sub"),
            background: false,
        })
        .expect("projection");
        assert_eq!(same.mode, ResolvedWorkflowChatProgressMode::LiveCard);
        assert_eq!(same.repo_relation, RepoRelation::Same);
        assert_eq!(same.repo_label.as_deref(), Some("repo-a"));

        // A non-repository workflow cwd: auto resolves off, no label, and the identity is None
        // (never "assume same").
        let other = resolve_workflow_chat_progress(ResolveWorkflowChatProgressInput {
            requested: None,
            parent_cwd: &repo_dir,
            workflow_cwd: &plain,
            background: false,
        })
        .expect("projection");
        assert_eq!(other.mode, ResolvedWorkflowChatProgressMode::Off);
        assert_eq!(other.repo_relation, RepoRelation::Other);
        assert_eq!(other.repo_label, None);
        assert!(resolve_git_repository_identity(&plain).is_none());
        assert!(!is_same_git_repository(&repo_dir, &plain));

        std::fs::remove_dir_all(&scratch).ok();
    }

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "cyrup-subagents-chat-progress-{}",
            uuid::Uuid::new_v4().as_simple()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    /// The identity comparison is `common_dir` OR `root` equality — `common_dir` is the
    /// load-bearing half: two linked WORKTREES of one repository have different roots and the
    /// SAME common dir, and comparing only roots would call them different repositories and
    /// silently disable the live card. And `None` on either side is `false`, never "assume
    /// same".
    #[test]
    fn linked_worktrees_compare_as_the_same_repository() {
        let shared_common = GitRepositoryIdentity {
            root: PathBuf::from("/checkouts/main"),
            common_dir: PathBuf::from("/checkouts/main/.git"),
        };
        let linked_worktree = GitRepositoryIdentity {
            root: PathBuf::from("/checkouts/feature-wt"),
            common_dir: PathBuf::from("/checkouts/main/.git"),
        };
        let other_repo = GitRepositoryIdentity {
            root: PathBuf::from("/checkouts/other"),
            common_dir: PathBuf::from("/checkouts/other/.git"),
        };
        assert!(is_same_git_repository_identity(
            Some(&shared_common),
            Some(&linked_worktree)
        ));
        assert!(!is_same_git_repository_identity(
            Some(&shared_common),
            Some(&other_repo)
        ));
        assert!(!is_same_git_repository_identity(Some(&shared_common), None));
        assert!(!is_same_git_repository_identity(None, None));
    }

    /// The four row-builder subtleties: `reused` updates an existing row's labels only (and never
    /// creates one), `preflight` is first-lane-wins, omitted `runId`/`durationMs`/`error` CLEAR,
    /// and `label`/`phase` assign only when present.
    #[test]
    fn row_builder_matches_upstreams_clearing_and_reuse_rules() {
        let lane_key = WorkflowKey::parse("lane").expect("valid");
        let lane = WorkflowPreflightLane {
            key: lane_key,
            mode: None,
            decision: Some("first".to_string()),
            claims: None,
            expected_output: None,
            independence: None,
        };
        let second_lane = WorkflowPreflightLane {
            decision: Some("second".to_string()),
            ..lane.clone()
        };
        let lanes = [lane.clone(), second_lane];
        let calls = std::cell::Cell::new(0usize);
        let lookup = |_key: &WorkflowKey, _generated: Option<&WorkflowKey>| {
            let index = calls.get();
            calls.set(index + 1);
            lanes.get(index).cloned()
        };
        let trace = vec![
            // A reused entry for an unknown key: no row is created.
            entry("lane.ghost", WorkflowScriptTraceState::Reused, |entry| {
                entry.label = Some("Ghost".to_string());
            }),
            entry("lane.a", WorkflowScriptTraceState::Started, |entry| {
                entry.run_id = Some("run-1".to_string());
                entry.label = Some("  Lane A  ".to_string());
            }),
            // Later entry omits runId (cleared) and supplies no label (kept).
            entry("lane.a", WorkflowScriptTraceState::Completed, |entry| {
                entry.duration_ms = Some(42);
            }),
            // A reused entry updates labels on the existing row without touching state.
            entry("lane.a", WorkflowScriptTraceState::Reused, |entry| {
                entry.phase = Some("verify".to_string());
                entry.label = Some("   ".to_string());
            }),
        ];
        let rows = build_workflow_chat_progress_rows(&trace, Some(&lookup));
        assert_eq!(rows.len(), 1, "reused never creates a row");
        let row = rows.first().expect("row");
        assert_eq!(row.state, WorkflowChatProgressRowState::Complete);
        assert_eq!(row.run_id, None, "an omitted runId CLEARS the stale one");
        assert_eq!(row.duration_ms, Some(42));
        assert_eq!(
            row.label.as_deref(),
            Some("Lane A"),
            "trimmed, and kept when omitted or blank"
        );
        assert_eq!(row.phase.as_deref(), Some("verify"), "reused updates phase");
        assert_eq!(
            row.preflight
                .as_ref()
                .and_then(|lane| lane.decision.as_deref()),
            Some("first"),
            "the first lane wins; the second lookup cannot overwrite it"
        );
    }
}
