//! `/subagent-cost` recursive token/cost usage accounting (func-SA §5.6 R-SA-140; arch-SA §6.8).
//!
//! # The dual-shape recursion requirement (R-SA-140), read literally
//!
//! > Token/cost accounting for `/subagent-cost` MUST sum usage recursively through nested
//! > subagent-of-subagent trees (both a run's `children` array and any per-step nested children
//! > within async chain jobs) — a flat single-level sum is non-conformant.
//!
//! arch-SA §6.8 restates this as: "`/subagent-cost` finds the latest session file by **mtime**,
//! not filename, parses the session's own JSONL entries plus every discovered child artifact
//! `_meta.json` under the run's artifact directory, applying **additive dual-recursion (children
//! array + per-step nested children)** before rendering."
//!
//! That sentence names two textually distinct shapes a nested subagent-of-subagent tree can be
//! encoded in on disk, and this module recurses through **both**, independently, summing into one
//! shared accumulator — never just one of them:
//!
//! 1. **The artifact `_meta.json` "children array" shape** ([`RunMetadata::children`]): a
//!    single-run/single-step artifact's own `_meta.json` (func-SA §4.7 `RunMetadata`) may itself
//!    declare a `children: Vec<RunMetadata>` array — this is how a synchronous, in-band
//!    subagent-of-subagent delegation (a subagent whose own child process itself spawned and
//!    waited on a further subagent, folding that grandchild's usage into its own artifact tree
//!    before exiting) surfaces its descendants' cost. [`accumulate_meta_tree`] walks this shape.
//! 2. **The per-step nested-children shape inside async chain jobs**
//!    ([`crate::background::StepStatus::nested_run_ids`]): a background/async chain run's `status.json`
//!    records, per step, the [`background::RunId`]s of any further background runs that step
//!    itself kicked off (func-SA §4.5's `StepStatus` "nested-child descriptors" —
//!    `background/mod.rs`'s own doc comment on `nested_run_ids` names this exact field as R-SA-104's
//!    nested-descendant list). Each such id resolves to its own [`background::RunPaths::nested`]
//!    sibling tree (its own `status.json`, and potentially its own `_meta.json` artifacts, and
//!    potentially further `nested_run_ids` of its own). [`accumulate_nested_run_ids`] walks this
//!    shape, recursing arbitrarily deep.
//!
//! An implementation that only walks shape 1 silently **drops** any subagent-of-subagent cost that
//! was incurred through a *background* nested run (no `_meta.json` "children" entry would ever
//! exist for it in the walking run's own artifact tree — it lives under a wholly separate
//! `RunPaths::nested` subtree keyed by its own run id). An implementation that only walks shape 2
//! silently **drops** any subagent-of-subagent cost incurred through a *synchronous, in-band*
//! nested delegation (no separate background run/`RunId` was ever minted for it — it never
//! appears in any `nested_run_ids` list, only inside the parent artifact's own `_meta.json`
//! `children` array). Per func-SA's own explicit warning text quoted above, recursing only one
//! shape is non-conformant — this module's [`compute_recursive_cost`] entry point always walks
//! both, additively, into the same [`CostUsage`] accumulator.
//!
//! # Ownership boundary
//!
//! This file owns exactly the recursive accounting algorithm and its minimal supporting on-disk
//! artifact schema (func-SA §4.7's `RunMetadata`/`RunArtifactPaths`, which no other file in this
//! crate defines yet as of this phase — see the doc comment on [`RunMetadata`] for the narrow scope
//! of this local definition). It does **not** own: the `/subagent-cost` command's argument
//! parsing/rendering (a later phase of `registration/slash_commands.rs`, not yet present in this
//! crate — this module exposes [`compute_recursive_cost`]/[`format_cost_report`] as the pure
//! functions that command handler will call once it exists); writing `_meta.json` in the first
//! place (a later phase of `exec/`/`background/runner_main.rs`'s own artifact-persistence work);
//! or `RunStatus`/`RunPaths` themselves (already defined in [`crate::background`], consumed here
//! read-only).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use cyrup_core::{Message, ModelId, Usage};
use cyrup_session::{AgentMessage, Entry, KnownEntry};

use crate::background::{RunId, RunPaths, RunStatus};
use crate::error::SubagentError;

// =================================================================================================
// CostUsage: an additive accumulator over cyrup_core::Usage/Cost
// =================================================================================================

/// An additive running total over [`cyrup_core::Usage`] (func-SA §4.7's `RunMetadata`
/// `Usage{input,output,cache_read,cache_write,cost,turns}` shape).
///
/// [`cyrup_core::Usage`] itself has no built-in accumulation method (unlike arch-SA §3.4's own
/// illustrative `exec::Usage::add` sketch, which is a *different*, exec-local `Usage` shape not
/// reused here — `exec::mod::SingleResult.usage` is `cyrup_core::Usage`, the crate's one real
/// usage type, per that module's own `use cyrup_core::{..., Usage}` import). This wrapper supplies
/// the missing additive-fold operation this module's recursion needs, without mutating or
/// extending `cyrup_core::Usage` itself (out of this file's ownership boundary).
///
/// Also tracks `turns` (func-SA §4.7 names this as part of the accounted `Usage` shape, but
/// `cyrup_core::Usage` itself has no `turns` field — turn-count is carried separately in
/// [`RunMetadata::turns`] and folded in here alongside the token/cost totals) and
/// `models_seen` (every distinct final/attempted model observed anywhere in the walked tree,
/// useful for a `/subagent-cost` report breaking totals down per model — R-SA-140 does not
/// mandate a per-model breakdown, but tracking the seen-set costs nothing extra during a
/// recursion that is already visiting every node).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CostUsage {
    /// Additive input-token total across every node summed so far.
    pub input: u64,
    /// Additive output-token total.
    pub output: u64,
    /// Additive cache-read-token total.
    pub cache_read: u64,
    /// Additive cache-write-token total.
    pub cache_write: u64,
    /// Additive dollar-cost total.
    pub cost: f64,
    /// Additive turn-count total.
    pub turns: u64,
    /// Total number of distinct run/step nodes folded into this accumulator (root plus every
    /// recursively-visited child, across both R-SA-140 shapes) — a diagnostic/report field, not
    /// itself part of the summed usage.
    pub node_count: u64,
    /// Every distinct model name observed across the walked tree, insertion-order-independent
    /// (a `HashSet` rather than `Vec` since the accumulator's purpose is "which models were
    /// involved", not "in what order").
    pub models_seen: HashSet<ModelId>,
}

impl CostUsage {
    /// Folds one node's own [`cyrup_core::Usage`] into this running total (additive — R-SA-140's
    /// "sum usage recursively", never a last-write-wins replacement). Does not touch
    /// [`Self::node_count`]/[`Self::models_seen`]/[`Self::turns`] — callers combine this with
    /// [`Self::record_node`] where a full [`RunMetadata`]/model is in scope.
    pub fn add_usage(&mut self, usage: &Usage) {
        self.input += usage.input;
        self.output += usage.output;
        self.cache_read += usage.cache_read;
        self.cache_write += usage.cache_write;
        self.cost += usage.cost.total;
    }

    /// Records that one additional run/step node was visited during the recursion, folding its
    /// `turns` count and (if present) its model name into the running totals. Called exactly once
    /// per node visited by [`accumulate_meta_tree`]/[`accumulate_nested_run_ids`], in addition to
    /// (not instead of) [`Self::add_usage`] for that same node's own [`Usage`].
    pub fn record_node(&mut self, turns: u64, model: Option<&ModelId>) {
        self.node_count += 1;
        self.turns += turns;
        if let Some(model) = model {
            self.models_seen.insert(model.clone());
        }
    }

    /// Merges `other` into `self`, additively across every field (including `node_count` and the
    /// union of `models_seen`). Used to combine the two independently-walked R-SA-140 shapes
    /// (`_meta.json` children-array totals and per-step `nested_run_ids` totals) into one final
    /// report without either shape's walk needing to know about the other's accumulator.
    pub fn merge(&mut self, other: &CostUsage) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
        self.cost += other.cost;
        self.turns += other.turns;
        self.node_count += other.node_count;
        for model in &other.models_seen {
            self.models_seen.insert(model.clone());
        }
    }
}

// =================================================================================================
// RunMetadata / RunArtifactPaths (func-SA §4.7) — minimal local definition
// =================================================================================================

/// The `_meta.json` artifact schema (func-SA §4.7's `RunMetadata`): "timing, `Usage{input,output,
/// cache_read,cache_write,cost,turns}`, exit code, final model, attempted-model/fallback history."
///
/// # Scope note
///
/// func-SA §4.7 and arch-SA §3.8/§4.3 document `RunMetadata`/`RunArtifactPaths` as part of this
/// crate's persistence surface, but as of this phase no other file in the crate (`exec/`,
/// `background/`) has yet defined or written this shape — the artifact-writing side (constructing
/// and persisting a `_meta.json` per subagent run/step, keyed by `RunArtifactPaths`) is later,
/// unassigned build-out for `exec/`/`background/runner_main.rs`, not this file. This module defines
/// the **read-side** shape it needs to parse an on-disk `_meta.json` and recurse through its
/// `children` array — kept deliberately minimal (only the fields R-SA-140's accounting actually
/// consumes, plus the `children` field the dual-recursion requirement is specifically about) rather
/// than speculatively modeling every field func-SA's prose lists, so this file does not silently
/// diverge from whatever exact shape the eventual writer settles on for fields this module never
/// reads. `#[serde(default)]` on every field (and `#[serde(deny_unknown_fields)]` deliberately
/// **omitted**) means this reader tolerates a `_meta.json` that carries additional fields this
/// module does not model, forward-compatibly.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RunMetadata {
    /// The agent name this artifact's run/step invoked, if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// This node's own token/cost usage (NOT including any `children`'s usage — the recursive
    /// summation in this module is what folds descendants in; a `RunMetadata` value's own `usage`
    /// field is always exactly one node's contribution).
    pub usage: Usage,
    /// Turn count for this node alone (func-SA §4.7 lists `turns` as part of the accounted
    /// `Usage{...,turns}` shape; `cyrup_core::Usage` itself has no such field, so it is carried
    /// here instead — see [`CostUsage`]'s own doc for why).
    pub turns: u64,
    /// Process exit code for this node's run, if it has finished.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// The model actually used for the attempt that finished (or is currently running).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelId>,
    /// Every model attempted for this node, in fallback-ladder order (mirrors
    /// `exec::fallback::ModelAttempt`'s own ordering convention, R-SA-038).
    pub attempted_models: Vec<ModelId>,
    /// **The dual-recursion "children array" shape (R-SA-140).** Any further subagent-of-subagent
    /// delegations this node's own run performed synchronously, in-band, before this artifact was
    /// written — e.g. a subagent whose own child process itself spawned and awaited a nested
    /// subagent and folded that grandchild's artifact into its own before exiting. Recursed by
    /// [`accumulate_meta_tree`]. Distinct from, and additive with, the separate
    /// [`crate::background::StepStatus::nested_run_ids`] shape (R-SA-140's other half), which this type
    /// deliberately does not attempt to also represent — a background nested run's cost lives
    /// under its own [`RunPaths::nested`] subtree, addressed by run id, not embedded here.
    pub children: Vec<RunMetadata>,
}

impl RunMetadata {
    /// Loads and parses one `_meta.json` file from disk. Tolerant of any well-formed JSON object
    /// matching this shape (extra unknown fields ignored, per the type's own `#[serde(default)]`
    /// policy) but propagates a [`SubagentError::Spawn`]-wrapped I/O error on a missing/unreadable
    /// file or a [`SubagentError::StructuredOutputInvalid`] on malformed JSON — mirroring
    /// `discovery/management.rs`'s own established `SubagentError::Spawn(std::io::Error)` /
    /// stringified-parse-error convention for filesystem-adjacent failures in this crate, rather
    /// than inventing a third error-wrapping idiom for this one file.
    ///
    /// # Errors
    ///
    /// Returns [`SubagentError::Spawn`] if the file cannot be read, or
    /// [`SubagentError::StructuredOutputInvalid`] if its contents are not valid JSON matching this
    /// shape.
    pub async fn load(path: &Path) -> Result<Self, SubagentError> {
        let bytes = tokio::fs::read(path).await.map_err(SubagentError::Spawn)?;
        serde_json::from_slice(&bytes).map_err(|e| {
            SubagentError::StructuredOutputInvalid(format!(
                "malformed _meta.json at {}: {e}",
                path.display()
            ))
        })
    }

    /// Attempts to load `_meta.json` at `path`, returning `Ok(None)` (rather than an error) when
    /// the file simply does not exist — the common case for a leaf artifact directory that never
    /// itself spawned further nested subagents, which is not an error condition for the R-SA-140
    /// walk (an absent `_meta.json` just means "zero additional children-array usage to fold in
    /// from this node", not a failure). Any *other* I/O error (permissions, a directory where a
    /// file was expected, etc.) or a malformed-but-present file still propagates, since those
    /// genuinely indicate something wrong rather than "nothing here".
    ///
    /// # Errors
    ///
    /// Returns an error for any read/parse failure other than the file simply not existing.
    pub async fn load_if_present(path: &Path) -> Result<Option<Self>, SubagentError> {
        match tokio::fs::metadata(path).await {
            Ok(_) => Self::load(path).await.map(Some),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(SubagentError::Spawn(e)),
        }
    }
}

/// Per-`(run_id, agent, index)` well-known artifact file paths (func-SA §4.7 `RunArtifactPaths`).
/// Kept minimal to exactly the one field this module's read-side walk needs
/// ([`Self::meta_json`]) — the sibling `input_md`/`output_md`/`transcript_jsonl` paths func-SA §4.7
/// also documents are part of the artifact-*writing* side's own concern (unowned by this file, see
/// [`RunMetadata`]'s scope note), not this recursive-accounting algorithm's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunArtifactPaths {
    /// `<artifact_dir>/_meta.json` — this node's own [`RunMetadata`].
    pub meta_json: PathBuf,
}

impl RunArtifactPaths {
    /// Derives the artifact paths for one run/step's artifact directory. Pure path arithmetic —
    /// never touches the filesystem.
    #[must_use]
    pub fn for_dir(artifact_dir: &Path) -> Self {
        Self {
            meta_json: artifact_dir.join("_meta.json"),
        }
    }
}

// =================================================================================================
// Shape 1: the `_meta.json` "children array" recursion
// =================================================================================================

/// Recursively sums usage through a [`RunMetadata`] tree's `children` array — R-SA-140's first
/// named shape ("a run's `children` array"). Pure, synchronous, and total: never panics, never
/// short-circuits on an empty `children` list (a leaf node still contributes its own `usage`/
/// `turns`/`model` to the accumulator before returning).
///
/// This function does **not** touch the filesystem — it operates purely over an already-loaded
/// [`RunMetadata`] value (which itself may have been assembled by recursively loading further
/// `_meta.json` files via [`load_meta_tree_from_dir`], the filesystem-walking counterpart).
/// Separating the pure fold from the I/O keeps this exact recursion — the part R-SA-140 and A-SA-17
/// are actually specifying/testing — independently unit-testable without a real filesystem fixture
/// for every case.
pub fn accumulate_meta_tree(meta: &RunMetadata) -> CostUsage {
    let mut acc = CostUsage::default();
    accumulate_meta_tree_into(meta, &mut acc);
    acc
}

/// The recursive worker behind [`accumulate_meta_tree`], folding into a caller-supplied
/// accumulator so [`compute_recursive_cost`] can combine this shape's totals with shape 2's
/// ([`accumulate_nested_run_ids`]) into one shared [`CostUsage`] without an intermediate
/// allocation/merge step for every recursive call.
fn accumulate_meta_tree_into(meta: &RunMetadata, acc: &mut CostUsage) {
    acc.add_usage(&meta.usage);
    acc.record_node(meta.turns, meta.model.as_ref());
    for child in &meta.children {
        accumulate_meta_tree_into(child, acc);
    }
}

/// Loads `_meta.json` from `artifact_dir` (if present) and recursively resolves any further
/// artifact directories its `children` might reference **by nested directory**, in addition to
/// whatever `children` the loaded `_meta.json` itself already carries inline.
///
/// # Two ways a "children array" can be realized on disk
///
/// A writer may either (a) embed a full nested [`RunMetadata`] value directly inline in the
/// parent's own `_meta.json` `children` array (the common case for a synchronous, in-process fold
/// performed before the parent artifact was written), or (b) — for a case where the nested
/// artifact was written to its own subdirectory rather than folded inline before the parent's own
/// write — leave the parent's `children` array empty/partial and instead rely on a directory-walk
/// convention: any immediate subdirectory of `artifact_dir` that itself contains a `_meta.json` is
/// also a child artifact. This function honors **both**: it starts from whatever `children` the
/// loaded `_meta.json` already carries inline (shape (a), already fully recursive via
/// [`RunMetadata::children`] itself), and *additionally* scans `artifact_dir` for child
/// subdirectories carrying their own `_meta.json` not already accounted for by an inline entry,
/// recursing into each. This directory-scan half is what makes the "children array" shape robust
/// to either persistence strategy a future artifact-writing phase might choose, rather than this
/// reader silently only supporting whichever one happens to get implemented first.
///
/// Returns `Ok(None)` if `artifact_dir` has no `_meta.json` of its own at all (nothing to
/// recurse from at this root) — mirrors [`RunMetadata::load_if_present`]'s "absence is not an
/// error" policy.
///
/// # Errors
///
/// Returns an error if a present `_meta.json` fails to parse, or on an I/O error while walking
/// `artifact_dir` for child subdirectories (other than the directory simply not existing).
pub async fn load_meta_tree_from_dir(
    artifact_dir: &Path,
) -> Result<Option<RunMetadata>, SubagentError> {
    let paths = RunArtifactPaths::for_dir(artifact_dir);
    let Some(mut meta) = RunMetadata::load_if_present(&paths.meta_json).await? else {
        return Ok(None);
    };

    // Directory-scan half: any immediate subdirectory carrying its own `_meta.json` that was NOT
    // already represented inline in `meta.children` is folded in too (Box::pin for the async
    // recursion — `load_meta_tree_from_dir` calling itself across an `.await` needs a heap-indirect
    // future, since an `async fn`'s own future cannot be infinitely-sized to contain itself).
    let mut entries = match tokio::fs::read_dir(artifact_dir).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Some(meta));
        }
        Err(e) => return Err(SubagentError::Spawn(e)),
    };

    // Track which subdirectory names are already represented by an inline `children` entry so the
    // directory-scan half never double-counts a node the `_meta.json` itself already embedded.
    // Inline children carry no directory-name field in this minimal schema, so the de-dup key used
    // here is coarser (agent name) than a hypothetical directory-name field would allow; a false
    // "already seen" match (two distinct nested runs sharing one agent name, one inline and one
    // directory-discovered) is the one accepted imprecision of this heuristic — documented rather
    // than silently assumed correct, and not a concern for R-SA-140's own conformance target (which
    // is "recurse both shapes", not "never double-count under a degenerate same-agent-name
    // collision").
    let already_inline: HashSet<Option<String>> =
        meta.children.iter().map(|c| c.agent.clone()).collect();

    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(SubagentError::Spawn)?
    {
        let path = entry.path();
        let is_dir = entry
            .file_type()
            .await
            .map(|ft| ft.is_dir())
            .unwrap_or(false);
        if !is_dir {
            continue;
        }
        let nested = Box::pin(load_meta_tree_from_dir(&path)).await?;
        if let Some(nested_meta) = nested {
            if already_inline.contains(&nested_meta.agent) && nested_meta.agent.is_some() {
                continue;
            }
            meta.children.push(nested_meta);
        }
    }

    Ok(Some(meta))
}

// =================================================================================================
// Shape 2: the per-step `nested_run_ids` recursion (async chain jobs)
// =================================================================================================

/// Recursively sums usage through the **second** R-SA-140 shape: "any per-step nested children
/// within async chain jobs" — [`crate::background::StepStatus::nested_run_ids`] on every step of a
/// [`RunStatus`], each resolved to its own nested [`RunPaths`] tree (its own `status.json`, whose
/// own steps may in turn carry further `nested_run_ids`, recursing arbitrarily deep — a
/// subagent-of-subagent-of-subagent chain of background runs, e.g. this module's own
/// three-level-deep test fixture).
///
/// For each resolved nested run, this function folds in:
/// - that nested run's own `status.json` (`RunStatus`) usage, via every one of *its* steps'
///   [`crate::background::StepStatus::usage`] fields (each step's own usage, plus recursing into that
///   step's own `nested_run_ids` in turn), and
/// - if a `_meta.json` artifact also exists in that nested run's directory, that artifact's own
///   children-array tree via [`load_meta_tree_from_dir`] — since a background run's own step may
///   ALSO have performed a synchronous, in-band nested delegation of its own (shape 1, nested
///   inside shape 2), and R-SA-140's dual-recursion requirement applies at every level of the
///   walk, not only at the root.
///
/// A [`RunId`] listed in `nested_run_ids` whose `status.json` no longer exists on disk (a nested
/// run directory that was pruned/never fully materialized) is treated as contributing zero
/// additional usage rather than as an error — a partially-cleaned-up run tree should not make the
/// whole `/subagent-cost` report fail; [`SubagentError`] is still returned for a *present-but-
/// malformed* `status.json`, since that indicates real corruption worth surfacing.
///
/// # Errors
///
/// Returns an error if a present nested `status.json` fails to parse, or on an unexpected I/O
/// error while reading it.
pub async fn accumulate_nested_run_ids(
    status: &RunStatus,
    own_paths: &RunPaths,
) -> Result<CostUsage, SubagentError> {
    let mut acc = CostUsage::default();
    accumulate_run_status_into(status, own_paths, &mut acc).await?;
    Ok(acc)
}

/// The recursive worker behind [`accumulate_nested_run_ids`] (and the top-level entry point
/// [`compute_recursive_cost`]): folds every step's own usage plus, for each step, both the
/// [`RunMetadata`] children-array shape (if that step's nested artifact directory carries one) and
/// this same recursion applied to every id in [`crate::background::StepStatus::nested_run_ids`].
async fn accumulate_run_status_into(
    status: &RunStatus,
    own_paths: &RunPaths,
    acc: &mut CostUsage,
) -> Result<(), SubagentError> {
    for step in &status.steps {
        acc.add_usage(&step.usage);
        // `StepStatus` (background/mod.rs) carries no explicit `turns` field of its own — turn
        // count for a step lives on the `_meta.json` artifact (if any) for that step, folded in
        // below via `load_meta_tree_from_dir`, not double-counted here.
        acc.record_node(0, step.model.as_ref());

        // Shape 1 nested inside shape 2: this step's own artifact directory (if the runner wrote
        // one) may itself carry a `_meta.json` "children array" tree — e.g. this step's agent
        // synchronously delegated to a further in-band subagent before the step's own artifact was
        // finalized. Best-effort: a step with no artifact directory at all is not an error.
        let step_artifact_dir = own_paths.run_dir.join("steps").join(step.agent.as_str());
        if let Some(meta) = load_meta_tree_from_dir(&step_artifact_dir).await? {
            accumulate_meta_tree_into(&meta, acc);
        }

        // Shape 2's own recursion: every background run this step itself spawned.
        for nested_run_id in &step.nested_run_ids {
            Box::pin(accumulate_one_nested_run(nested_run_id, own_paths, acc)).await?;
        }
    }

    // `parallel_groups` (func-SA §4.5) holds the per-child status of any `ParallelGroup`/
    // `DynamicGroup` step, each entry itself a full `StepStatus` — R-SA-140's "per-step nested
    // children" applies identically to a parallel-group child step as to a top-level chain step,
    // so these are walked with the same per-step logic (usage + artifact meta-tree + nested
    // run ids), not skipped.
    if let Some(groups) = &status.parallel_groups {
        for group in groups {
            for step in &group.children {
                acc.add_usage(&step.usage);
                acc.record_node(0, step.model.as_ref());

                let step_artifact_dir = own_paths.run_dir.join("steps").join(step.agent.as_str());
                if let Some(meta) = load_meta_tree_from_dir(&step_artifact_dir).await? {
                    accumulate_meta_tree_into(&meta, acc);
                }

                for nested_run_id in &step.nested_run_ids {
                    Box::pin(accumulate_one_nested_run(nested_run_id, own_paths, acc)).await?;
                }
            }
        }
    }

    Ok(())
}

/// Resolves one [`RunId`] found in a [`crate::background::StepStatus::nested_run_ids`] list to its
/// nested [`RunPaths`] (via [`RunPaths::nested`]), loads that nested run's own `status.json` (if
/// present — absence contributes zero, per this module's documented tolerance), and recursively
/// folds its usage (both shapes, at every level) into `acc`.
async fn accumulate_one_nested_run(
    nested_run_id: &RunId,
    parent_paths: &RunPaths,
    acc: &mut CostUsage,
) -> Result<(), SubagentError> {
    let nested_paths = parent_paths.nested(nested_run_id);

    match tokio::fs::read(&nested_paths.status).await {
        Ok(bytes) => {
            let nested_status: RunStatus = serde_json::from_slice(&bytes).map_err(|e| {
                SubagentError::StructuredOutputInvalid(format!(
                    "malformed nested status.json for run {nested_run_id} at {}: {e}",
                    nested_paths.status.display()
                ))
            })?;
            Box::pin(accumulate_run_status_into(&nested_status, &nested_paths, acc)).await?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Pruned/never-materialized nested run directory: contributes zero, not an error
            // (see this function's own doc comment / accumulate_nested_run_ids's doc).
        }
        Err(e) => return Err(SubagentError::Spawn(e)),
    }

    // Even with no (or an unreadable-but-absent) nested status.json, the nested run's own
    // artifact directory might still carry a top-level `_meta.json` children-array tree (e.g. a
    // short-lived synchronous-only nested run that never became a tracked async status at all,
    // yet still left an artifact). Fold that in too, so a run's total is never silently
    // understated purely because `status.json` itself is missing.
    if let Some(meta) = load_meta_tree_from_dir(&nested_paths.run_dir).await? {
        accumulate_meta_tree_into(&meta, acc);
    }

    Ok(())
}

// =================================================================================================
// Top-level entry point: additive dual-recursion combining both shapes
// =================================================================================================

/// The `/subagent-cost` accounting entry point (R-SA-140; arch-SA §6.8's "Token accounting
/// reload"): given one run's resolved [`RunStatus`] and [`RunPaths`], computes the total recursive
/// usage across **both** R-SA-140 shapes, additively combined into one [`CostUsage`] — never just
/// one shape, per this module's own top-of-file warning.
///
/// Concretely:
/// 1. Walks the run's own top-level artifact directory ([`RunPaths::run_dir`]) for a `_meta.json`
///    children-array tree ([`load_meta_tree_from_dir`] + [`accumulate_meta_tree`]) — shape 1.
/// 2. Walks every step's [`crate::background::StepStatus::nested_run_ids`] (and, for steps inside a
///    `ParallelGroup`/`DynamicGroup`, every parallel-group child's own `nested_run_ids`),
///    recursing into each nested background run's own status/artifact tree, at every level
///    ([`accumulate_nested_run_ids`]) — shape 2, which itself recurses into shape 1 at every
///    nested level too (a nested background run's own step may have its own synchronous,
///    in-band artifact children).
/// 3. Also folds the top-level run's own step usage directly (steps 1 and 2 above only cover the
///    run's *artifact tree* and *nested runs*; the run's own `status.json` steps' own `usage`
///    fields are the base case this recursion is built on, not something layered on top of it —
///    handled by delegating the whole walk to `accumulate_run_status_into`, which folds a
///    step's own usage before ever looking at that step's children).
///
/// The result is one [`CostUsage`] whose `input`/`output`/`cache_read`/`cache_write`/`cost`/`turns`
/// totals are the additive sum over: the root run's own steps, every step's synchronous
/// `_meta.json` children (recursively), and every step's background `nested_run_ids` (recursively,
/// including each nested run's own steps and *their* children/nested runs in turn) — a true,
/// unbounded-depth, dual-shape recursive sum, per A-SA-17's own "includes the grandchild's usage,
/// not just the immediate child's" acceptance bar (and beyond: a great-grandchild, however deep the
/// tree actually goes, is included identically, since `accumulate_one_nested_run`/
/// [`load_meta_tree_from_dir`] recurse via `Box::pin`-wrapped self-calls with no depth cap).
///
/// # Errors
///
/// Returns an error if any *present* `status.json`/`_meta.json` in the walked tree fails to parse,
/// or on an unexpected (non-"not found") I/O error. A run/artifact that is simply absent
/// contributes zero usage rather than erroring — see `accumulate_one_nested_run`'s doc.
pub async fn compute_recursive_cost(
    status: &RunStatus,
    run_paths: &RunPaths,
) -> Result<CostUsage, SubagentError> {
    let mut acc = CostUsage::default();

    // Shape 2 (plus the base-case step usage every walk is built on): the root run's own steps,
    // their artifact children, and their nested background runs, recursively.
    accumulate_run_status_into(status, run_paths, &mut acc).await?;

    // Shape 1 at the ROOT level: the run's own top-level artifact directory may itself carry a
    // `_meta.json` describing the run as a whole (distinct from any individual step's own
    // per-step artifact directory, which `accumulate_run_status_into` already handles) — e.g. a
    // single (non-chain) run's own top-level synchronous nested-delegation tree.
    if let Some(root_meta) = load_meta_tree_from_dir(&run_paths.run_dir).await? {
        accumulate_meta_tree_into(&root_meta, &mut acc);
    }

    Ok(acc)
}

// =================================================================================================
// Latest-session-file-by-mtime lookup (arch-SA §6.8: "finds the latest session file by mtime, not
// filename")
// =================================================================================================

/// Finds the most recently **modified** (by `mtime`, never by filename lexical/numeric ordering)
/// `.jsonl` file directly inside `session_dir` — arch-SA §6.8's explicit instruction for how
/// `/subagent-cost` locates "the" current session file to report on. Filename-based ordering is
/// deliberately rejected as a substitute: session filenames in this codebase are not guaranteed to
/// sort chronologically (e.g. a resumed/forked session's filename carries no timestamp component
/// this module can rely on), so `mtime` is the only correct signal.
///
/// Returns `Ok(None)` if `session_dir` contains no `.jsonl` files at all (not an error — an empty
/// or not-yet-populated session directory is a valid, reportable "nothing to show yet" state for
/// the eventual `/subagent-cost` command handler to render, not a failure of this lookup).
///
/// # Errors
///
/// Returns an error on a genuine I/O failure reading `session_dir` or a file's metadata (other than
/// the directory simply not existing, which is treated identically to "no `.jsonl` files found").
pub async fn find_latest_session_file_by_mtime(
    session_dir: &Path,
) -> Result<Option<PathBuf>, SubagentError> {
    let mut entries = match tokio::fs::read_dir(session_dir).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(SubagentError::Spawn(e)),
    };

    let mut latest: Option<(PathBuf, std::time::SystemTime)> = None;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(SubagentError::Spawn)?
    {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let metadata = entry.metadata().await.map_err(SubagentError::Spawn)?;
        if !metadata.is_file() {
            continue;
        }
        let modified = metadata.modified().map_err(SubagentError::Spawn)?;

        let replace = match &latest {
            None => true,
            Some((_, latest_mtime)) => modified > *latest_mtime,
        };
        if replace {
            latest = Some((path, modified));
        }
    }

    Ok(latest.map(|(path, _)| path))
}

// =================================================================================================
// `/subagent-cost` session-transcript walk (pi `buildSubagentCostReport`, slash-commands.ts:377-416)
//
// This is the shape `/subagent-cost` actually renders (R-SA-140's user-facing surface), and it is a
// DIFFERENT computation from the recursive background-artifact accumulator above: pi's cost command
// walks the *session transcript* (`ctx.sessionManager.getBranch()`), summing the parent's own
// assistant-message usage plus a per-child breakdown of every subagent `toolResult` recorded in the
// branch — so foreground subagent usage (which never produces a background run/`status.json` at all)
// is visible. The recursive `compute_recursive_cost` accumulator remains a separate, independently
// useful capability (nested background-run cost), but it is not what a user sees from
// `/subagent-cost`.
// =================================================================================================

/// The custom-message `customType` a slash-invoked subagent result is stored under in the session
/// transcript (pi `SLASH_RESULT_TYPE`, shared/types.ts:963) — its `details.result.details` payload
/// carries the same `{mode, results}` subagent-details shape a tool-invoked subagent stores directly
/// on its `toolResult` message.
const SLASH_RESULT_TYPE: &str = "subagent-slash-result";

/// pi's local `Usage` accounting shape for the cost report (shared/types.ts `Usage`:
/// `{input, output, cacheRead, cacheWrite, cost, turns}`) — deliberately DISTINCT from
/// [`cyrup_core::Usage`] (whose `cost` is a nested `Cost{total}` and which has no `turns` field) and
/// from [`CostUsage`] (the recursive accumulator above). This is the flat, render-oriented total the
/// `formatCostUsage` line renders: additive across the walked branch, one field per rendered column.
#[derive(Clone, Debug, Default, PartialEq)]
struct TranscriptUsage {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    cost: f64,
    turns: u64,
}

impl TranscriptUsage {
    /// Projects one [`cyrup_core::Usage`] plus a caller-supplied `turns` count into this flat shape
    /// (pi reads `usage.cost.total` for the cost column; `turns` is carried separately since
    /// `cyrup_core::Usage` has no such field — a parent assistant message contributes `turns: 1`
    /// like pi's `assistantUsageFromMessage`, a child result contributes `turns: 0` since cyrup's
    /// per-child `Usage` records no turn count, an honest divergence from pi's turn-carrying child
    /// usage that the gap analysis notes as an agreed usage-turn-counting deferral).
    fn from_core(usage: &Usage, turns: u64) -> Self {
        Self {
            input: usage.input,
            output: usage.output,
            cache_read: usage.cache_read,
            cache_write: usage.cache_write,
            cost: usage.cost.total,
            turns,
        }
    }

    /// Additive fold (pi `addUsage`, slash-commands.ts:315-322) — every column summed, never
    /// last-write-wins.
    fn add(&mut self, other: &TranscriptUsage) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
        self.cost += other.cost;
        self.turns += other.turns;
    }

    /// pi `usageHasValue` (slash-commands.ts:324-326): a child result is only listed when at least
    /// one accounting column is non-zero, so a zero-usage tool result never adds an empty "Child N"
    /// line.
    fn has_value(&self) -> bool {
        self.input != 0
            || self.output != 0
            || self.cache_read != 0
            || self.cache_write != 0
            || self.cost != 0.0
            || self.turns != 0
    }
}

/// One `{agent, usage, sessionFile?}` child entry parsed out of a subagent `toolResult`'s
/// `details.results` array (pi `SingleResult` subset the cost walk reads, shared/types.ts:803-892).
struct TranscriptChild {
    agent: String,
    usage: Usage,
    session_file: Option<String>,
}

/// pi `formatTokens` (shared/formatters.ts): `< 1000` renders the raw integer, `< 10000` renders one
/// decimal place with a `k` suffix, otherwise a rounded-thousands `k`.
fn format_tokens(n: u64) -> String {
    if n < 1000 {
        n.to_string()
    } else if n < 10_000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        format!("{}k", (n as f64 / 1000.0).round() as u64)
    }
}

/// pi `formatCostUsage` (slash-commands.ts:368-375): `"{label}: ↑{in} ↓{out} ${cost}(...extras)"`,
/// where extras (cache read / cache write / turns) are only appended when non-zero.
fn format_cost_usage(label: &str, usage: &TranscriptUsage) -> String {
    let mut extras: Vec<String> = Vec::new();
    if usage.cache_read != 0 {
        extras.push(format!("cache read {}", format_tokens(usage.cache_read)));
    }
    if usage.cache_write != 0 {
        extras.push(format!("cache write {}", format_tokens(usage.cache_write)));
    }
    if usage.turns != 0 {
        extras.push(format!(
            "{} turn{}",
            usage.turns,
            if usage.turns == 1 { "" } else { "s" }
        ));
    }
    let extra = if extras.is_empty() {
        String::new()
    } else {
        format!(" ({})", extras.join(", "))
    };
    format!(
        "{label}: ↑{} ↓{} ${:.4}{extra}",
        format_tokens(usage.input),
        format_tokens(usage.output),
        usage.cost
    )
}

/// pi `assistantUsageFromMessage` (slash-commands.ts:328-347): the parent's own per-turn usage for a
/// `role: "assistant"` message. Returns the message's [`cyrup_core::Usage`] (the cost column reads
/// its `cost.total`); the caller folds it in with `turns: 1`.
fn assistant_usage_from_entry(entry: &Entry) -> Option<&Usage> {
    match entry {
        Entry::Known(KnownEntry::Message {
            message: AgentMessage::Core(Message::Assistant(assistant)),
            ..
        }) => Some(&assistant.usage),
        _ => None,
    }
}

/// pi `isSubagentDetails` (slash-commands.ts:349-353) + the per-result field reads of
/// `buildSubagentCostReport`: a details value is subagent details only when it is an object carrying
/// a string `mode` AND an array `results`. Each result's `agent`/`usage`/`sessionFile` is read
/// leniently (matching pi's untyped field access), so a malformed individual result degrades to
/// zero usage rather than discarding the whole details object.
fn parse_subagent_details(details: &serde_json::Value) -> Option<Vec<TranscriptChild>> {
    let obj = details.as_object()?;
    if !obj.get("mode").is_some_and(serde_json::Value::is_string) {
        return None;
    }
    let results = obj.get("results")?.as_array()?;
    let children = results
        .iter()
        .map(|result| {
            let agent = result
                .get("agent")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let usage = result
                .get("usage")
                .and_then(|value| serde_json::from_value::<Usage>(value.clone()).ok())
                .unwrap_or_default();
            let session_file = result
                .get("sessionFile")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            TranscriptChild {
                agent,
                usage,
                session_file,
            }
        })
        .collect();
    Some(children)
}

/// pi `detailsFromSessionEntry` (slash-commands.ts:355-366): extract subagent `{mode, results}`
/// details from a session entry, whether stored directly on a `toolResult` message whose `toolName`
/// is `subagent` (the tool-invoked path) or nested under `details.result.details` of a
/// [`SLASH_RESULT_TYPE`] custom message (the slash-invoked path).
fn details_from_session_entry(entry: &Entry) -> Option<Vec<TranscriptChild>> {
    match entry {
        Entry::Known(KnownEntry::CustomMessage {
            custom_type,
            details,
            ..
        }) if custom_type == SLASH_RESULT_TYPE => {
            let inner = details
                .as_ref()?
                .get("result")
                .and_then(|result| result.get("details"))?;
            parse_subagent_details(inner)
        }
        Entry::Known(KnownEntry::Message {
            message: AgentMessage::Core(Message::ToolResult {
                tool_name, details, ..
            }),
            ..
        }) if tool_name == "subagent" => parse_subagent_details(details.as_ref()?),
        _ => None,
    }
}

/// Build the `/subagent-cost` report by walking one session-transcript branch (pi
/// `buildSubagentCostReport`, slash-commands.ts:377-416), root→leaf. Sums the parent's own
/// assistant-message usage and a per-child breakdown of every subagent `toolResult` in the branch,
/// then renders pi's exact multi-line report (Parent line, per-child lines with their optional
/// `Session:` reference, a divider, the Children subtotal, and the grand Total).
///
/// `branch` is the ordered entry sequence a caller obtains from
/// [`cyrup_session::SessionManager::branch_path`] (the cyrup analog of pi's
/// `ctx.sessionManager.getBranch()`); an empty branch renders the well-formed "no child usage"
/// report rather than an error.
#[must_use]
pub fn build_subagent_cost_report<'a>(branch: impl IntoIterator<Item = &'a Entry>) -> String {
    let mut parent = TranscriptUsage::default();
    let mut child_total = TranscriptUsage::default();
    let mut children: Vec<(String, TranscriptUsage, Option<String>)> = Vec::new();

    for entry in branch {
        if let Some(usage) = assistant_usage_from_entry(entry) {
            parent.add(&TranscriptUsage::from_core(usage, 1));
        }
        let Some(results) = details_from_session_entry(entry) else {
            continue;
        };
        for child in results {
            let usage = TranscriptUsage::from_core(&child.usage, 0);
            if !usage.has_value() {
                continue;
            }
            let label = format!("Child {} ({})", children.len() + 1, child.agent);
            child_total.add(&usage);
            children.push((label, usage, child.session_file));
        }
    }

    let mut total = TranscriptUsage::default();
    total.add(&parent);
    total.add(&child_total);

    let mut lines = vec![
        "Subagent cost".to_string(),
        String::new(),
        format_cost_usage("Parent", &parent),
    ];
    if children.is_empty() {
        lines.push("No subagent child usage found in this session.".to_string());
    } else {
        for (label, usage, session_file) in &children {
            lines.push(format_cost_usage(label, usage));
            if let Some(session_file) = session_file {
                lines.push(format!("  Session: {session_file}"));
            }
        }
    }
    lines.push("────────────────────────────".to_string());
    lines.push(format_cost_usage("Children", &child_total));
    lines.push(format_cost_usage("Total", &total));
    lines.join("\n")
}

// =================================================================================================
// CostReport: a small rendering-ready summary (consumed by a later slash-command-handler phase)
// =================================================================================================

/// A rendering-ready summary of one [`compute_recursive_cost`] result — the shape a later
/// `/subagent-cost` command-handler phase (`registration/slash_commands.rs`, not yet present in
/// this crate) is expected to format for terminal display. Kept here (rather than invented ad hoc
/// by that future phase) since it is a pure, trivial projection of [`CostUsage`] with no
/// additional accounting logic of its own — one canonical shape, not two independently-derived
/// summaries that could drift.
#[derive(Clone, Debug, PartialEq)]
pub struct CostReport {
    pub run_id: RunId,
    pub usage: CostUsage,
}

/// Computes a full [`CostReport`] for one run, combining [`compute_recursive_cost`] with the
/// run's own identity. The thin convenience wrapper a command handler calls end-to-end.
///
/// # Errors
///
/// Propagates any error from [`compute_recursive_cost`].
pub async fn build_cost_report(
    status: &RunStatus,
    run_paths: &RunPaths,
) -> Result<CostReport, SubagentError> {
    let usage = compute_recursive_cost(status, run_paths).await?;
    Ok(CostReport {
        run_id: status.run_id.clone(),
        usage,
    })
}

/// Renders a [`CostReport`] as a compact, human-readable multi-line string — a minimal, dependency-
/// free formatter so this module is independently useful/testable without waiting on a future
/// TUI-facing renderer. Not itself a stable wire format; a later phase's actual `/subagent-cost`
/// output MAY reformat this however it likes — this function exists so [`compute_recursive_cost`]'s
/// output has at least one concrete, testable rendering today.
#[must_use]
pub fn format_cost_report(report: &CostReport) -> String {
    let u = &report.usage;
    let mut models: Vec<&str> = u.models_seen.iter().map(ModelId::as_str).collect();
    models.sort_unstable();
    format!(
        "run {}: {} node(s), {} turn(s)\n\
         tokens: input={} output={} cache_read={} cache_write={}\n\
         cost: ${:.4}\n\
         models: {}",
        report.run_id,
        u.node_count,
        u.turns,
        u.input,
        u.output,
        u.cache_read,
        u.cache_write,
        u.cost,
        if models.is_empty() {
            "(none)".to_string()
        } else {
            models.join(", ")
        }
    )
}

// =================================================================================================
// Tests
// =================================================================================================

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use crate::background::{RunMode, RunState, StepState, StepStatus};
    use cyrup_core::Cost as CoreCost;

    // ---------------------------------------------------------------------------------------
    // Test fixtures
    // ---------------------------------------------------------------------------------------

    fn usage(input: u64, output: u64, cost_total: f64) -> Usage {
        Usage {
            input,
            output,
            cache_read: 0,
            cache_write: 0,
            cache_write_1h: None,
            reasoning: None,
            total_tokens: input + output,
            cost: CoreCost {
                input: 0.0,
                output: 0.0,
                cache_read: 0.0,
                cache_write: 0.0,
                total: cost_total,
            },
        }
    }

    fn leaf_meta(agent: &str, input: u64, output: u64, cost_total: f64, turns: u64) -> RunMetadata {
        RunMetadata {
            agent: Some(agent.to_string()),
            usage: usage(input, output, cost_total),
            turns,
            exit_code: Some(0),
            model: Some(ModelId::from(format!("{agent}-model"))),
            attempted_models: vec![ModelId::from(format!("{agent}-model"))],
            children: Vec::new(),
        }
    }

    // ---------------------------------------------------------------------------------------
    // Shape 1 (`_meta.json` children array): pure recursion over an in-memory RunMetadata tree
    // ---------------------------------------------------------------------------------------

    #[test]
    fn accumulate_meta_tree_leaf_node_contributes_only_its_own_usage() {
        let meta = leaf_meta("worker", 100, 50, 0.01, 3);
        let acc = accumulate_meta_tree(&meta);

        assert_eq!(acc.input, 100);
        assert_eq!(acc.output, 50);
        assert!((acc.cost - 0.01).abs() < f64::EPSILON);
        assert_eq!(acc.turns, 3);
        assert_eq!(acc.node_count, 1);
        assert_eq!(acc.models_seen.len(), 1);
    }

    #[test]
    fn accumulate_meta_tree_three_levels_deep_includes_grandchild_usage() {
        // researcher -> reviewer -> fact-checker: a genuine 3-level-deep subagent-of-subagent
        // tree (parent -> child -> grandchild), entirely via the "children array" shape.
        let grandchild = leaf_meta("fact-checker", 10, 5, 0.001, 1);
        let child = RunMetadata {
            agent: Some("reviewer".to_string()),
            usage: usage(40, 20, 0.004),
            turns: 2,
            exit_code: Some(0),
            model: Some(ModelId::from("reviewer-model")),
            attempted_models: vec![ModelId::from("reviewer-model")],
            children: vec![grandchild],
        };
        let root = RunMetadata {
            agent: Some("researcher".to_string()),
            usage: usage(200, 100, 0.02),
            turns: 5,
            exit_code: Some(0),
            model: Some(ModelId::from("researcher-model")),
            attempted_models: vec![ModelId::from("researcher-model")],
            children: vec![child],
        };

        let acc = accumulate_meta_tree(&root);

        // Manual addition of each level's usage (the task's explicit verification method):
        // root:       input=200 output=100 cost=0.02  turns=5
        // child:      input=40  output=20  cost=0.004 turns=2
        // grandchild: input=10  output=5   cost=0.001 turns=1
        let expected_input = 200 + 40 + 10;
        let expected_output = 100 + 20 + 5;
        let expected_cost = 0.02 + 0.004 + 0.001;
        let expected_turns = 5 + 2 + 1;

        assert_eq!(acc.input, expected_input, "grandchild input must be included");
        assert_eq!(acc.output, expected_output, "grandchild output must be included");
        assert!(
            (acc.cost - expected_cost).abs() < 1e-9,
            "grandchild cost must be included: got {}, expected {}",
            acc.cost,
            expected_cost
        );
        assert_eq!(acc.turns, expected_turns);
        assert_eq!(acc.node_count, 3, "root + child + grandchild = 3 nodes");
        assert_eq!(
            acc.models_seen.len(),
            3,
            "three distinct models, one per level"
        );
    }

    #[test]
    fn accumulate_meta_tree_wide_fanout_sums_every_sibling() {
        // One parent with THREE children (not nested further) - proves siblings are summed, not
        // just a single linear chain.
        let root = RunMetadata {
            agent: Some("orchestrator".to_string()),
            usage: usage(1000, 500, 0.1),
            turns: 1,
            exit_code: Some(0),
            model: Some(ModelId::from("orchestrator-model")),
            attempted_models: vec![],
            children: vec![
                leaf_meta("a", 10, 10, 0.001, 1),
                leaf_meta("b", 20, 20, 0.002, 1),
                leaf_meta("c", 30, 30, 0.003, 1),
            ],
        };

        let acc = accumulate_meta_tree(&root);

        assert_eq!(acc.input, 1000 + 10 + 20 + 30);
        assert_eq!(acc.output, 500 + 10 + 20 + 30);
        assert!((acc.cost - (0.1 + 0.001 + 0.002 + 0.003)).abs() < 1e-9);
        assert_eq!(acc.node_count, 4);
    }

    // ---------------------------------------------------------------------------------------
    // CostUsage::merge — combining the two independently-walked shapes
    // ---------------------------------------------------------------------------------------

    #[test]
    fn cost_usage_merge_is_additive_across_both_accumulators() {
        let mut a = CostUsage::default();
        a.add_usage(&usage(10, 20, 1.0));
        a.record_node(2, Some(&ModelId::from("model-a")));

        let mut b = CostUsage::default();
        b.add_usage(&usage(5, 5, 0.5));
        b.record_node(1, Some(&ModelId::from("model-b")));

        a.merge(&b);

        assert_eq!(a.input, 15);
        assert_eq!(a.output, 25);
        assert!((a.cost - 1.5).abs() < f64::EPSILON);
        assert_eq!(a.turns, 3);
        assert_eq!(a.node_count, 2);
        assert_eq!(a.models_seen.len(), 2);
    }

    // ---------------------------------------------------------------------------------------
    // Shape 1, filesystem-backed: load_meta_tree_from_dir over real temp-dir fixtures
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn load_meta_tree_from_dir_returns_none_when_no_meta_json_present() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let result = load_meta_tree_from_dir(dir.path())
            .await
            .expect("no I/O error for an empty dir");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn load_meta_tree_from_dir_reads_inline_children_array() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let meta = RunMetadata {
            agent: Some("root".to_string()),
            usage: usage(100, 50, 0.01),
            turns: 1,
            exit_code: Some(0),
            model: Some(ModelId::from("root-model")),
            attempted_models: vec![],
            children: vec![leaf_meta("child", 10, 5, 0.001, 1)],
        };
        let paths = RunArtifactPaths::for_dir(dir.path());
        write_atomic_test_json(&paths.meta_json, &meta).await;

        let loaded = load_meta_tree_from_dir(dir.path())
            .await
            .expect("loads")
            .expect("meta.json present");

        assert_eq!(loaded.children.len(), 1);
        let acc = accumulate_meta_tree(&loaded);
        assert_eq!(acc.input, 110);
        assert_eq!(acc.node_count, 2);
    }

    #[tokio::test]
    async fn load_meta_tree_from_dir_also_discovers_child_meta_json_in_subdirectories() {
        // The directory-scan half of shape 1: a nested subdirectory carrying its own `_meta.json`
        // that was NOT inlined into the parent's own `children` array.
        let dir = tempfile::tempdir().expect("real tempdir");
        let root_meta = RunMetadata {
            agent: Some("root".to_string()),
            usage: usage(100, 50, 0.01),
            turns: 1,
            exit_code: Some(0),
            model: None,
            attempted_models: vec![],
            children: Vec::new(), // deliberately empty inline
        };
        let root_paths = RunArtifactPaths::for_dir(dir.path());
        write_atomic_test_json(&root_paths.meta_json, &root_meta).await;

        let child_dir = dir.path().join("nested-child");
        tokio::fs::create_dir_all(&child_dir)
            .await
            .expect("mkdir child dir");
        let child_meta = leaf_meta("nested-child-agent", 7, 3, 0.0007, 1);
        let child_paths = RunArtifactPaths::for_dir(&child_dir);
        write_atomic_test_json(&child_paths.meta_json, &child_meta).await;

        let loaded = load_meta_tree_from_dir(dir.path())
            .await
            .expect("loads")
            .expect("root meta present");

        assert_eq!(
            loaded.children.len(),
            1,
            "directory-discovered child must be folded into children"
        );
        let acc = accumulate_meta_tree(&loaded);
        assert_eq!(acc.input, 107);
        assert_eq!(acc.node_count, 2);
    }

    // ---------------------------------------------------------------------------------------
    // Shape 2, filesystem-backed: accumulate_nested_run_ids over real RunPaths/status.json
    // fixtures — the 3-level-deep nested-subagent-of-subagent test the task explicitly requires.
    // ---------------------------------------------------------------------------------------

    fn step_with_usage(agent: &str, input: u64, output: u64, cost_total: f64) -> StepStatus {
        StepStatus {
            agent: agent.to_string(),
            status: StepState::Complete,
            session_file: None,
            model: Some(ModelId::from(format!("{agent}-model"))),
            attempted_models: vec![ModelId::from(format!("{agent}-model"))],
            usage: usage(input, output, cost_total),
            error: None,
            nested_run_ids: Vec::new(),
            started_at: Some(0),
            ended_at: Some(1),
            telemetry: crate::background::StepTelemetry::default(),
        }
    }

    fn run_status_single_step(run_id: RunId, step: StepStatus) -> RunStatus {
        let mut status = RunStatus::queued(run_id, RunMode::Chain, Some(1));
        status.state = RunState::Complete;
        status.steps = vec![step];
        status
    }

    async fn write_atomic_test_json<T: serde::Serialize + Sync>(path: &Path, value: &T) {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .expect("mkdir -p parent");
        }
        crate::background::atomic::write_atomic_json(path, value)
            .await
            .expect("atomic write for test fixture");
    }

    /// Builds a real, on-disk 3-level-deep nested-subagent-of-subagent fixture entirely through
    /// shape 2 (background `nested_run_ids` + real `status.json` files under `RunPaths::nested`):
    ///
    /// - **root run** (`root_id`): one step (`researcher`, usage A) whose `nested_run_ids`
    ///   references `child_id`.
    /// - **child run** (`child_id`, nested under root): one step (`reviewer`, usage B) whose
    ///   `nested_run_ids` references `grandchild_id`.
    /// - **grandchild run** (`grandchild_id`, nested under child): one step (`fact-checker`,
    ///   usage C), no further nesting.
    ///
    /// Returns `(root_status, root_paths, expected_totals)` where `expected_totals` is the
    /// manually-added-by-hand sum of A + B + C, computed independently of the code under test, per
    /// the task's explicit "verified against manual addition of each level's usage" requirement.
    async fn build_three_level_nested_fixture(
        tmp: &Path,
    ) -> (RunStatus, RunPaths, (u64, u64, f64)) {
        let async_root = tmp.join("async-root");
        let results_dir = tmp.join("results");

        let root_id = RunId::from_token("root0000000000000000000000000001");
        let child_id = RunId::from_token("child00000000000000000000000001");
        let grandchild_id = RunId::from_token("gchild0000000000000000000000001");

        let root_paths = RunPaths::for_run(&async_root, &results_dir, &root_id);
        let child_paths = root_paths.nested(&child_id);
        let grandchild_paths = child_paths.nested(&grandchild_id);

        // usage A (root's own step), B (child's own step), C (grandchild's own step) —
        // deliberately distinct, easy-to-hand-add numbers.
        let usage_a = (300u64, 150u64, 0.03f64); // researcher
        let usage_b = (120u64, 60u64, 0.012f64); // reviewer
        let usage_c = (40u64, 20u64, 0.004f64); // fact-checker

        // Grandchild: leaf run, no further nesting.
        let grandchild_step = step_with_usage("fact-checker", usage_c.0, usage_c.1, usage_c.2);
        let grandchild_status =
            run_status_single_step(grandchild_id.clone(), grandchild_step);
        tokio::fs::create_dir_all(&grandchild_paths.run_dir)
            .await
            .expect("mkdir grandchild run dir");
        write_atomic_test_json(&grandchild_paths.status, &grandchild_status).await;

        // Child: one step that itself nests the grandchild run.
        let mut child_step = step_with_usage("reviewer", usage_b.0, usage_b.1, usage_b.2);
        child_step.nested_run_ids = vec![grandchild_id.clone()];
        let child_status = run_status_single_step(child_id.clone(), child_step);
        tokio::fs::create_dir_all(&child_paths.run_dir)
            .await
            .expect("mkdir child run dir");
        write_atomic_test_json(&child_paths.status, &child_status).await;

        // Root: one step that nests the child run.
        let mut root_step = step_with_usage("researcher", usage_a.0, usage_a.1, usage_a.2);
        root_step.nested_run_ids = vec![child_id.clone()];
        let root_status = run_status_single_step(root_id.clone(), root_step);
        tokio::fs::create_dir_all(&root_paths.run_dir)
            .await
            .expect("mkdir root run dir");
        write_atomic_test_json(&root_paths.status, &root_status).await;

        let expected_input = usage_a.0 + usage_b.0 + usage_c.0;
        let expected_output = usage_a.1 + usage_b.1 + usage_c.1;
        let expected_cost = usage_a.2 + usage_b.2 + usage_c.2;

        (
            root_status,
            root_paths,
            (expected_input, expected_output, expected_cost),
        )
    }

    #[tokio::test]
    async fn accumulate_nested_run_ids_three_levels_deep_includes_grandchild_usage() {
        let tmp = tempfile::tempdir().expect("real tempdir");
        let (root_status, root_paths, (expected_input, expected_output, expected_cost)) =
            build_three_level_nested_fixture(tmp.path()).await;

        let acc = accumulate_nested_run_ids(&root_status, &root_paths)
            .await
            .expect("recursion succeeds over real fixture files");

        assert_eq!(
            acc.input, expected_input,
            "grandchild's input usage must be included, not just the immediate child's"
        );
        assert_eq!(
            acc.output, expected_output,
            "grandchild's output usage must be included, not just the immediate child's"
        );
        assert!(
            (acc.cost - expected_cost).abs() < 1e-9,
            "grandchild's cost must be included: got {}, expected {}",
            acc.cost,
            expected_cost
        );
        // root step + child step + grandchild step = 3 nodes visited.
        assert_eq!(acc.node_count, 3);
    }

    #[tokio::test]
    async fn accumulate_nested_run_ids_flat_single_level_matches_flat_sum_but_is_not_a_regression_case()
     {
        // A degenerate but important control case: with NO nested_run_ids anywhere, the
        // recursive walk must reduce to exactly the flat, single-level sum (proving the
        // recursion doesn't spuriously inflate totals when there is nothing to recurse into).
        let tmp = tempfile::tempdir().expect("real tempdir");
        let async_root = tmp.path().join("async-root");
        let results_dir = tmp.path().join("results");
        let run_id = RunId::from_token("flatrun00000000000000000000001");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);

        let step = step_with_usage("solo-worker", 50, 25, 0.005);
        let status = run_status_single_step(run_id, step);
        tokio::fs::create_dir_all(&paths.run_dir)
            .await
            .expect("mkdir");
        write_atomic_test_json(&paths.status, &status).await;

        let acc = accumulate_nested_run_ids(&status, &paths)
            .await
            .expect("recursion succeeds");

        assert_eq!(acc.input, 50);
        assert_eq!(acc.output, 25);
        assert!((acc.cost - 0.005).abs() < 1e-9);
        assert_eq!(acc.node_count, 1);
    }

    #[tokio::test]
    async fn accumulate_one_nested_run_missing_status_json_contributes_zero_not_an_error() {
        let tmp = tempfile::tempdir().expect("real tempdir");
        let async_root = tmp.path().join("async-root");
        let results_dir = tmp.path().join("results");
        let root_id = RunId::from_token("pruned0000000000000000000000001");
        let missing_child_id = RunId::from_token("gone000000000000000000000000001");

        let mut root_step = step_with_usage("researcher", 10, 10, 0.001);
        root_step.nested_run_ids = vec![missing_child_id];
        let root_status = run_status_single_step(root_id.clone(), root_step);
        let root_paths = RunPaths::for_run(&async_root, &results_dir, &root_id);
        tokio::fs::create_dir_all(&root_paths.run_dir)
            .await
            .expect("mkdir");
        write_atomic_test_json(&root_paths.status, &root_status).await;
        // Deliberately never create the nested child's run directory/status.json at all.

        let acc = accumulate_nested_run_ids(&root_status, &root_paths)
            .await
            .expect("a pruned/never-materialized nested run must not error the whole walk");

        assert_eq!(acc.input, 10, "only the root's own step usage is counted");
        assert_eq!(acc.node_count, 1);
    }

    // ---------------------------------------------------------------------------------------
    // compute_recursive_cost — the combined dual-shape entry point (both shapes together)
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn compute_recursive_cost_combines_both_shapes_additively() {
        // Root run has: (a) a background-nested child via `nested_run_ids` (shape 2) carrying
        // usage B, and (b) its OWN top-level `_meta.json` children-array entry (shape 1) carrying
        // usage D — proving BOTH shapes are summed together, not just one.
        let tmp = tempfile::tempdir().expect("real tempdir");
        let async_root = tmp.path().join("async-root");
        let results_dir = tmp.path().join("results");

        let root_id = RunId::from_token("dualshape000000000000000000001");
        let child_id = RunId::from_token("dualchild0000000000000000000001");

        let root_paths = RunPaths::for_run(&async_root, &results_dir, &root_id);
        let child_paths = root_paths.nested(&child_id);

        let usage_a = (100u64, 50u64, 0.01f64); // root's own step (base case)
        let usage_b = (20u64, 10u64, 0.002f64); // shape-2 nested background child
        let usage_d = (5u64, 5u64, 0.0005f64); // shape-1 synchronous meta-tree child

        // Shape 2: nested background child run.
        let child_step = step_with_usage("bg-child", usage_b.0, usage_b.1, usage_b.2);
        let child_status = run_status_single_step(child_id.clone(), child_step);
        tokio::fs::create_dir_all(&child_paths.run_dir)
            .await
            .expect("mkdir child");
        write_atomic_test_json(&child_paths.status, &child_status).await;

        // Root's own step references the shape-2 child.
        let mut root_step = step_with_usage("root-agent", usage_a.0, usage_a.1, usage_a.2);
        root_step.nested_run_ids = vec![child_id];
        let root_status = run_status_single_step(root_id, root_step);
        tokio::fs::create_dir_all(&root_paths.run_dir)
            .await
            .expect("mkdir root");
        write_atomic_test_json(&root_paths.status, &root_status).await;

        // Shape 1: the root's own top-level `_meta.json` with an inline synchronous child.
        let root_meta = RunMetadata {
            agent: Some("root-agent".to_string()),
            usage: usage(0, 0, 0.0), // avoid double counting the base-case step usage
            turns: 0,
            exit_code: Some(0),
            model: None,
            attempted_models: vec![],
            children: vec![leaf_meta("sync-child", usage_d.0, usage_d.1, usage_d.2, 1)],
        };
        let root_artifact_paths = RunArtifactPaths::for_dir(&root_paths.run_dir);
        write_atomic_test_json(&root_artifact_paths.meta_json, &root_meta).await;

        let acc = compute_recursive_cost(&root_status, &root_paths)
            .await
            .expect("combined recursion succeeds");

        let expected_input = usage_a.0 + usage_b.0 + usage_d.0;
        let expected_output = usage_a.1 + usage_b.1 + usage_d.1;
        let expected_cost = usage_a.2 + usage_b.2 + usage_d.2;

        assert_eq!(
            acc.input, expected_input,
            "both shape-1 (meta children) and shape-2 (nested_run_ids) usage must be summed"
        );
        assert_eq!(acc.output, expected_output);
        assert!(
            (acc.cost - expected_cost).abs() < 1e-9,
            "got {}, expected {}",
            acc.cost,
            expected_cost
        );
    }

    #[tokio::test]
    async fn build_cost_report_and_format_produce_a_nonempty_human_readable_summary() {
        let tmp = tempfile::tempdir().expect("real tempdir");
        let async_root = tmp.path().join("async-root");
        let results_dir = tmp.path().join("results");
        let run_id = RunId::from_token("reportrun000000000000000000001");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        let step = step_with_usage("worker", 10, 5, 0.001);
        let status = run_status_single_step(run_id, step);
        tokio::fs::create_dir_all(&paths.run_dir)
            .await
            .expect("mkdir");
        write_atomic_test_json(&paths.status, &status).await;

        let report = build_cost_report(&status, &paths)
            .await
            .expect("builds report");
        let rendered = format_cost_report(&report);

        assert!(rendered.contains("tokens:"));
        assert!(rendered.contains("cost:"));
        assert!(rendered.contains("worker-model"));
    }

    // ---------------------------------------------------------------------------------------
    // find_latest_session_file_by_mtime
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn find_latest_session_file_by_mtime_returns_none_for_empty_dir() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let result = find_latest_session_file_by_mtime(dir.path())
            .await
            .expect("no error for empty dir");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn find_latest_session_file_by_mtime_returns_none_for_missing_dir() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let missing = dir.path().join("does-not-exist");
        let result = find_latest_session_file_by_mtime(&missing)
            .await
            .expect("missing dir is not an error");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn find_latest_session_file_by_mtime_picks_the_most_recently_modified_file_not_the_lexically_last_name()
     {
        let dir = tempfile::tempdir().expect("real tempdir");

        // Deliberately name the OLDER file so it would sort lexically LAST (e.g. "z-old.jsonl" >
        // "a-new.jsonl" alphabetically), proving the lookup uses mtime, never filename order.
        let older = dir.path().join("z-old.jsonl");
        let newer = dir.path().join("a-new.jsonl");

        tokio::fs::write(&older, b"{}\n").await.expect("write older");
        // Ensure a real, observable mtime gap on filesystems with coarse mtime resolution.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        tokio::fs::write(&newer, b"{}\n").await.expect("write newer");

        let found = find_latest_session_file_by_mtime(dir.path())
            .await
            .expect("finds a result")
            .expect("at least one .jsonl file present");

        assert_eq!(
            found, newer,
            "must pick the file with the later mtime, regardless of lexical filename order"
        );
    }

    #[tokio::test]
    async fn find_latest_session_file_by_mtime_ignores_non_jsonl_files() {
        let dir = tempfile::tempdir().expect("real tempdir");
        tokio::fs::write(dir.path().join("notes.txt"), b"irrelevant")
            .await
            .expect("write");
        tokio::fs::write(dir.path().join("session.jsonl"), b"{}\n")
            .await
            .expect("write");

        let found = find_latest_session_file_by_mtime(dir.path())
            .await
            .expect("finds")
            .expect("the one jsonl file");

        assert_eq!(found, dir.path().join("session.jsonl"));
    }

    // ---------------------------------------------------------------------------------------
    // RunMetadata::load / load_if_present
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn run_metadata_load_if_present_returns_none_for_missing_file() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let missing = dir.path().join("_meta.json");
        let result = RunMetadata::load_if_present(&missing)
            .await
            .expect("missing file is not an error");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn run_metadata_load_rejects_malformed_json() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let path = dir.path().join("_meta.json");
        tokio::fs::write(&path, b"{ not valid json")
            .await
            .expect("write malformed file");

        let result = RunMetadata::load(&path).await;
        assert!(matches!(
            result,
            Err(SubagentError::StructuredOutputInvalid(_))
        ));
    }

    #[tokio::test]
    async fn run_metadata_round_trips_through_json_including_children() {
        let meta = RunMetadata {
            agent: Some("root".to_string()),
            usage: usage(1, 2, 0.1),
            turns: 3,
            exit_code: Some(0),
            model: Some(ModelId::from("m")),
            attempted_models: vec![ModelId::from("m")],
            children: vec![leaf_meta("child", 4, 5, 0.2, 1)],
        };
        let json = serde_json::to_string(&meta).expect("serializes");
        let back: RunMetadata = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(back, meta);
    }

    #[tokio::test]
    async fn run_metadata_deserializes_with_missing_optional_fields_via_defaults() {
        // `#[serde(default)]` at the struct level: a minimal _meta.json missing most fields
        // still parses, with an empty children array (not an error) — this is what makes a leaf
        // artifact with no nested delegations at all a valid, common-case document.
        let minimal = r#"{"usage": {"input": 1, "output": 1, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 2, "cost": {"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0,"total":0.0}}}"#;
        let meta: RunMetadata = serde_json::from_str(minimal).expect("parses minimal doc");
        assert!(meta.children.is_empty());
        assert!(meta.agent.is_none());
    }

    // ---------------------------------------------------------------------------------------
    // `/subagent-cost` session-transcript walk (pi `buildSubagentCostReport`)
    // ---------------------------------------------------------------------------------------

    /// A `cyrup_core::Usage` (camelCase, nested `cost.total`) JSON value, the shape a real session
    /// stores for both an assistant message's own usage and a subagent child result's usage.
    fn usage_json(input: u64, output: u64, cache_read: u64, cache_write: u64, cost: f64) -> serde_json::Value {
        serde_json::json!({
            "input": input,
            "output": output,
            "cacheRead": cache_read,
            "cacheWrite": cache_write,
            "totalTokens": input + output,
            "cost": { "input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": cost },
        })
    }

    /// Deserialize one on-disk session-entry JSON line into a real [`Entry`] — the exact wire format
    /// `SessionManager` persists, so this walk is exercised over genuine entries, not a mock.
    fn entry(line: serde_json::Value) -> Entry {
        serde_json::from_value(line).expect("valid session entry")
    }

    fn assistant_entry(id: &str, parent: Option<&str>, usage: serde_json::Value) -> Entry {
        entry(serde_json::json!({
            "type": "message",
            "id": id,
            "parentId": parent,
            "timestamp": "2026-01-01T00:00:00.000Z",
            "message": {
                "role": "assistant",
                "content": [{ "type": "text", "text": "ok" }],
                "provider": "anthropic",
                "model": "claude-sonnet-4",
                "usage": usage,
                "stopReason": "stop",
                "timestamp": 1,
            },
        }))
    }

    fn subagent_tool_result_entry(
        id: &str,
        parent: Option<&str>,
        details: serde_json::Value,
    ) -> Entry {
        entry(serde_json::json!({
            "type": "message",
            "id": id,
            "parentId": parent,
            "timestamp": "2026-01-01T00:00:00.000Z",
            "message": {
                "role": "toolResult",
                "toolCallId": "call-1",
                "toolName": "subagent",
                "content": [{ "type": "text", "text": "done" }],
                "details": details,
                "timestamp": 2,
            },
        }))
    }

    #[test]
    fn cost_report_walks_transcript_and_sums_parent_plus_child_usage() {
        // One parent assistant turn (usage A) + one subagent toolResult carrying two child results
        // (usage B and C). The report must sum parent + both children, list each child, and show a
        // Children subtotal and a grand Total — verified against manual addition.
        let branch = [
            entry(serde_json::json!({
                "type": "message",
                "id": "u0000001",
                "parentId": null,
                "timestamp": "2026-01-01T00:00:00.000Z",
                "message": { "role": "user", "content": [{ "type": "text", "text": "go" }], "timestamp": 0 },
            })),
            assistant_entry("a0000001", Some("u0000001"), usage_json(200, 100, 0, 0, 0.02)),
            subagent_tool_result_entry(
                "t0000001",
                Some("a0000001"),
                serde_json::json!({
                    "mode": "parallel",
                    "results": [
                        { "agent": "worker", "usage": usage_json(50, 25, 0, 0, 0.005), "sessionFile": "/tmp/child-1.jsonl" },
                        { "agent": "reviewer", "usage": usage_json(30, 15, 0, 0, 0.003) },
                    ],
                }),
            ),
        ];

        let report = build_subagent_cost_report(branch.iter());

        // Structure.
        assert!(report.starts_with("Subagent cost\n"), "report: {report}");
        assert!(report.contains("Child 1 (worker)"), "report: {report}");
        assert!(report.contains("Child 2 (reviewer)"), "report: {report}");
        assert!(
            report.contains("  Session: /tmp/child-1.jsonl"),
            "a child carrying a sessionFile must render its Session reference: {report}"
        );

        // Sums (manual addition): parent input 200; children 50+30=80; total 280. Outputs: parent
        // 100; children 25+15=40; total 140. All below 1000 so formatTokens is the raw integer.
        assert!(report.contains("Parent: ↑200 ↓100"), "report: {report}");
        assert!(report.contains("Children: ↑80 ↓40"), "report: {report}");
        assert!(report.contains("Total: ↑280 ↓140"), "report: {report}");
        // Grand total cost 0.02 + 0.005 + 0.003 = 0.028, rendered to 4 dp.
        assert!(report.contains("$0.0280"), "grand total cost must sum parent+children: {report}");
        // Parent turn count folds in as 1 turn (assistantUsageFromMessage's `turns: 1`).
        assert!(report.contains("(1 turn)"), "parent turn count must render: {report}");
    }

    #[test]
    fn cost_report_empty_transcript_reports_no_child_usage() {
        let report = build_subagent_cost_report(std::iter::empty::<&Entry>());
        assert!(report.starts_with("Subagent cost\n"));
        assert!(
            report.contains("No subagent child usage found in this session."),
            "report: {report}"
        );
        assert!(report.contains("Parent: ↑0 ↓0 $0.0000"), "report: {report}");
        assert!(report.contains("Total: ↑0 ↓0 $0.0000"), "report: {report}");
    }

    #[test]
    fn cost_report_ignores_non_subagent_tool_results_and_zero_usage_children() {
        // A toolResult from a DIFFERENT tool must not be counted; a subagent result whose usage is
        // all-zero must not produce a child line (pi `usageHasValue`).
        let other_tool = entry(serde_json::json!({
            "type": "message",
            "id": "x0000001",
            "parentId": null,
            "timestamp": "2026-01-01T00:00:00.000Z",
            "message": {
                "role": "toolResult",
                "toolCallId": "c",
                "toolName": "read",
                "content": [{ "type": "text", "text": "file" }],
                "details": { "mode": "single", "results": [{ "agent": "nope", "usage": usage_json(999, 999, 0, 0, 9.0) }] },
                "timestamp": 1,
            },
        }));
        let zero_child = subagent_tool_result_entry(
            "t0000002",
            Some("x0000001"),
            serde_json::json!({
                "mode": "single",
                "results": [{ "agent": "idle", "usage": usage_json(0, 0, 0, 0, 0.0) }],
            }),
        );

        let report = build_subagent_cost_report([&other_tool, &zero_child]);
        assert!(
            report.contains("No subagent child usage found in this session."),
            "a non-subagent toolResult and a zero-usage subagent child must both be ignored: {report}"
        );
        assert!(!report.contains("nope"), "the `read` tool result must not be counted: {report}");
    }

    #[test]
    fn cost_report_reads_slash_result_custom_message() {
        // The slash-invoked path: a SLASH_RESULT_TYPE custom message nests its subagent details
        // under details.result.details (pi `detailsFromSessionEntry` custom_message arm).
        let custom = entry(serde_json::json!({
            "type": "custom_message",
            "id": "s0000001",
            "parentId": null,
            "timestamp": "2026-01-01T00:00:00.000Z",
            "customType": "subagent-slash-result",
            "content": "Subagent finished",
            "display": true,
            "details": {
                "requestId": "req-1",
                "result": {
                    "content": [{ "type": "text", "text": "done" }],
                    "details": {
                        "mode": "single",
                        "results": [{ "agent": "scout", "usage": usage_json(12, 8, 0, 0, 0.001) }],
                    },
                },
            },
        }));

        let report = build_subagent_cost_report([&custom]);
        assert!(report.contains("Child 1 (scout)"), "report: {report}");
        assert!(report.contains("Children: ↑12 ↓8"), "report: {report}");
    }

    #[test]
    fn format_tokens_matches_pi_thresholds() {
        assert_eq!(format_tokens(0), "0");
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(1500), "1.5k");
        assert_eq!(format_tokens(9999), "10.0k");
        assert_eq!(format_tokens(12_345), "12k");
    }
}

