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
//!    records, per step, the [`crate::background::RunId`]s of any further background runs that step
//!    itself kicked off (func-SA §4.5's `StepStatus` "nested-child descriptors" —
//!    `background/mod.rs`'s own doc comment on `nested_run_ids` names this exact field as R-SA-104's
//!    nested-descendant list). Each such id resolves to its own [`crate::background::RunPaths::nested`]
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
use crate::formatters::format_tokens;

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

    while let Some(entry) = entries.next_entry().await.map_err(SubagentError::Spawn)? {
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
        acc.record_node(step.turns, step.model.as_ref());

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
                acc.record_node(step.turns, step.model.as_ref());

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
            Box::pin(accumulate_run_status_into(
                &nested_status,
                &nested_paths,
                acc,
            ))
            .await?;
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
    while let Some(entry) = entries.next_entry().await.map_err(SubagentError::Spawn)? {
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
// `/subagent-cost` and the RPC `cost` method (pi `collectSubagentCost`, `slash/subagent-cost.ts`
// @v0.71.0, `858661af`/#2378)
//
// This is the shape `/subagent-cost` actually renders (R-SA-140's user-facing surface), and it is a
// DIFFERENT computation from the recursive background-artifact accumulator above: pi's cost command
// walks the *session transcript* (`ctx.sessionManager.getBranch()`), summing the parent's own
// assistant-message and compaction usage plus a per-child breakdown of every `subagent`/`bg_wait`
// tool result recorded in the branch — so foreground subagent usage (which never produces a
// background run/`status.json` at all) is visible — and then resolves async WORKFLOW children
// through their receipts and artifact metadata. One collector feeds both surfaces upstream: the
// slash command formats [`SubagentCostReport`], and the RPC `cost` method returns it as data.
// =================================================================================================

/// The custom-message `customType` a slash-invoked subagent result is stored under in the session
/// transcript (pi `SLASH_RESULT_TYPE`, shared/types.ts:963) — its `details.result.details` payload
/// carries the same `{mode, results}` subagent-details shape a tool-invoked subagent stores directly
/// on its `toolResult` message.
const SLASH_RESULT_TYPE: &str = "subagent-slash-result";

/// pi `SUBAGENT_COST_REPORT_VERSION` (`subagent-cost.ts:30`) — advertised as
/// `ping.capabilities.cost.version` and stamped on every report.
pub const SUBAGENT_COST_REPORT_VERSION: u32 = 1;

/// pi `MAX_USAGE_METADATA_BYTES` (`subagent-cost.ts:83`): an artifact `_meta.json` larger than this
/// is not read.
const MAX_USAGE_METADATA_BYTES: u64 = 2 * 1024 * 1024;

/// JavaScript's `Number.MAX_SAFE_INTEGER`, the bound of `Number.isSafeInteger` (`:67`).
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// pi-subagents' own six-field `Usage` (`shared/types.ts` `Usage`:
/// `{input, output, cacheRead, cacheWrite, cost, turns}`) — deliberately DISTINCT from
/// [`cyrup_core::Usage`] (whose `cost` is a nested `Cost{total}` and which has no `turns` field) and
/// from [`CostUsage`] (the recursive accumulator above). Serialized exactly as upstream's report
/// carries it, so an RPC client reads the same keys.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentCostUsage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cost: f64,
    pub turns: u64,
}

impl SubagentCostUsage {
    /// Additive fold (pi `addUsage`, `subagent-cost.ts:37-44`) — every column summed.
    fn add(&mut self, other: &Self) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
        self.cost += other.cost;
        self.turns += other.turns;
    }

    /// pi `usageHasValue` (`:46-48`): a child is only listed when at least one column is non-zero.
    fn has_value(&self) -> bool {
        self.input != 0
            || self.output != 0
            || self.cache_read != 0
            || self.cache_write != 0
            || self.cost != 0.0
            || self.turns != 0
    }
}

/// One listed child (pi `SubagentCostChild`, `subagent-cost.ts:22-28`).
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentCostChild {
    /// `Child <n> (<agent | "unknown">)`.
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub usage: SubagentCostUsage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_file: Option<String>,
}

/// Structured parent-plus-child accounting for one session (pi `SubagentCostReport`,
/// `subagent-cost.ts:11-20`): the source of `/subagent-cost` and the RPC `cost` method.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentCostReport {
    pub version: u32,
    pub parent: SubagentCostUsage,
    pub children: Vec<SubagentCostChild>,
    pub child_total: SubagentCostUsage,
    pub total: SubagentCostUsage,
    /// Async children whose usage metadata could not be resolved; the child total is a lower bound
    /// when this is non-zero.
    pub unresolved_async_children: u64,
}

/// Where [`collect_subagent_cost`] looks beyond the transcript — pi's `ctx.cwd`,
/// `ctx.sessionManager.getSessionFile()`, `state.baseCwd`, `state.artifactDirPreference` and
/// `DIRS.async` (`subagent-cost.ts:182-201`).
#[derive(Clone, Copy, Debug)]
pub struct SubagentCostSources<'a> {
    /// The persisted session file, when there is one — it roots the `session` artifact preference.
    pub session_file: Option<&'a Path>,
    /// The request's working directory (`ctx.cwd`).
    pub cwd: &'a Path,
    /// The extension's base working directory (`state.baseCwd`).
    pub base_cwd: &'a Path,
    /// `config.artifactDir`.
    pub artifact_dir_preference: crate::artifacts::ArtifactDirPreference,
    /// The async run root a workflow run id resolves under (`DIRS.async`).
    pub async_root: &'a Path,
}

/// pi `nonNegativeNumber` (`subagent-cost.ts:50-52`).
fn non_negative_number(value: Option<&serde_json::Value>) -> Option<f64> {
    value
        .and_then(serde_json::Value::as_f64)
        .filter(|number| number.is_finite() && *number >= 0.0)
}

/// A token column: a non-negative number, else `0`. Fractional counts truncate — JSON token counts
/// are integers on every producer.
fn token_column(value: Option<&serde_json::Value>) -> u64 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    non_negative_number(value).map_or(0, |number| number as u64)
}

/// pi `usageFromValue` (`subagent-cost.ts:54-68`): the six columns off a usage record, reading
/// `cost.total` when `cost` is an object. `turns_override` replaces the record's own `turns`; a
/// turn count that is not a safe integer rejects the whole record (`Number.isSafeInteger`).
fn usage_from_value(
    value: Option<&serde_json::Value>,
    turns_override: Option<&serde_json::Value>,
) -> Option<SubagentCostUsage> {
    let record = value?.as_object()?;
    let cost = match record.get("cost") {
        Some(serde_json::Value::Object(cost)) => cost.get("total"),
        other => other,
    };
    let turns = non_negative_number(turns_override.or_else(|| record.get("turns"))).unwrap_or(0.0);
    if turns.fract() != 0.0 || turns > MAX_SAFE_INTEGER {
        return None;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some(SubagentCostUsage {
        input: token_column(record.get("input")),
        output: token_column(record.get("output")),
        cache_read: token_column(record.get("cacheRead")),
        cache_write: token_column(record.get("cacheWrite")),
        cost: non_negative_number(cost).unwrap_or(0.0),
        turns: turns as u64,
    })
}

/// pi `assistantUsageFromMessage` (`:70-74`) or `compactionUsageFromEntry` (`:76-80`): an
/// assistant message counts one turn, a compaction's summarizing call counts none.
fn parent_usage_from_entry(entry: &Entry) -> Option<SubagentCostUsage> {
    let (usage, turns) = match entry {
        Entry::Known(KnownEntry::Message {
            message: AgentMessage::Core(Message::Assistant(assistant)),
            ..
        }) => (serde_json::to_value(&assistant.usage).ok()?, 1),
        Entry::Known(KnownEntry::Compaction {
            usage: Some(usage), ..
        }) => (serde_json::to_value(usage).ok()?, 0),
        _ => return None,
    };
    usage_from_value(Some(&usage), Some(&serde_json::Value::from(turns)))
}

/// pi `isSubagentDetails` (`subagent-cost.ts:105-109` @ad11b7ab): an object with a string `mode`
/// and an array `results`.
///
/// **[CYRUP-DELTA, representation]** A foreground workflow's details are also accepted when they
/// carry `mode: "workflow"` and an array `children` but no `results`. Upstream flattens each
/// child's per-round results into `details.results` (`workflowDetailsResults`,
/// `src/runs/foreground/subagent-executor.ts:4758-4764`, used at `:6625` and `:6657` @ad11b7ab);
/// cyrup's details keep them on `children[].results` instead (`workflow_launch.rs`), and
/// [`foreground_workflow_results`] does the same flatten here.
fn subagent_details(
    value: &serde_json::Value,
) -> Option<&serde_json::Map<String, serde_json::Value>> {
    let details = value.as_object()?;
    let mode = details.get("mode").and_then(serde_json::Value::as_str)?;
    let has_results = details
        .get("results")
        .is_some_and(serde_json::Value::is_array);
    let has_workflow_children = mode == "workflow"
        && details
            .get("children")
            .is_some_and(serde_json::Value::is_array);
    (has_results || has_workflow_children).then_some(details)
}

/// pi `workflowDetailsResults` (`src/runs/foreground/subagent-executor.ts:4758-4764` @ad11b7ab)
/// over cyrup's foreground workflow `details.children`: every child's per-round `results`,
/// flattened, each taking the child's `runId` only when the round names none of its own.
///
/// Each round keeps its own run id, so every round of a resumed child is a distinct `run:<id>`
/// identity and counts (#2612); a round without one shares the child's. A cyrup foreground
/// `SingleResult` spells its run id `childRunId` and leaves it unset for a foreground child, so
/// both spellings are read before the child's id is used.
fn foreground_workflow_results(
    details: &serde_json::Map<String, serde_json::Value>,
) -> Vec<(Option<String>, &serde_json::Value)> {
    if details.get("mode").and_then(serde_json::Value::as_str) != Some("workflow") {
        return Vec::new();
    }
    details
        .get("children")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|child| {
            let child_run_id = string_field(child, "runId");
            child
                .get("results")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .map(move |result| {
                    let run_id = string_field(result, "runId")
                        .or_else(|| string_field(result, "childRunId"))
                        .or_else(|| child_run_id.clone());
                    (run_id, result)
                })
        })
        .collect()
}

/// pi `detailsFromSessionEntry` (`:110-120`): subagent details stored directly on a `subagent` or
/// `bg_wait` tool result, or nested under `details.result.details` of a [`SLASH_RESULT_TYPE`]
/// custom message.
fn details_from_session_entry(
    entry: &Entry,
) -> Option<&serde_json::Map<String, serde_json::Value>> {
    match entry {
        Entry::Known(KnownEntry::CustomMessage {
            custom_type,
            details,
            ..
        }) if custom_type == SLASH_RESULT_TYPE => subagent_details(
            details
                .as_ref()?
                .get("result")
                .and_then(|result| result.get("details"))?,
        ),
        Entry::Known(KnownEntry::Message {
            message:
                AgentMessage::Core(Message::ToolResult {
                    tool_name, details, ..
                }),
            ..
        }) if tool_name == "subagent"
            || tool_name == crate::extension::wait_tool::WAIT_TOOL_NAME =>
        {
            subagent_details(details.as_ref()?)
        }
        _ => None,
    }
}

/// A string field, when it is one.
fn string_field(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// pi `readUsageMetadata` (`:85-102`): a regular file of at most [`MAX_USAGE_METADATA_BYTES`]
/// holding a JSON object. The size is checked on the open handle before anything is read.
fn read_usage_metadata(
    path: &Path,
) -> std::io::Result<Option<serde_json::Map<String, serde_json::Value>>> {
    use std::io::Read as _;
    let file = std::fs::File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_USAGE_METADATA_BYTES {
        return Ok(None);
    }
    let mut buffer = Vec::new();
    file.take(MAX_USAGE_METADATA_BYTES)
        .read_to_end(&mut buffer)?;
    Ok(serde_json::from_slice::<serde_json::Value>(&buffer)
        .ok()
        .and_then(|value| match value {
            serde_json::Value::Object(map) => Some(map),
            _ => None,
        }))
}

/// pi `metadataUsage` (`subagent-cost.ts:124-145` @ad11b7ab): the first artifact `_meta.json` —
/// each of `indexes` in order (upstream's default is index `0`, then unindexed:
/// [`DEFAULT_METADATA_INDEXES`]), in each artifacts directory — whose `runId` and `agent` match and
/// whose usage is non-zero.
fn metadata_usage(
    artifacts_dirs: &[PathBuf],
    run_id: &str,
    agent: &str,
    indexes: &[Option<usize>],
) -> Option<SubagentCostUsage> {
    if run_id.is_empty()
        || !run_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return None;
    }
    for dir in artifacts_dirs {
        for &index in indexes {
            let path = crate::artifacts::artifact_paths(dir, run_id, agent, index).metadata_path;
            match read_usage_metadata(&path) {
                Ok(Some(metadata)) => {
                    if metadata.get("runId").and_then(serde_json::Value::as_str) != Some(run_id)
                        || metadata.get("agent").and_then(serde_json::Value::as_str) != Some(agent)
                    {
                        continue;
                    }
                    if let Some(usage) = usage_from_value(metadata.get("usage"), None)
                        && usage.has_value()
                    {
                        return Some(usage);
                    }
                }
                Ok(None) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => tracing::warn!(
                    target: "cyrup_ext_subagents::cost",
                    path = %path.display(),
                    %error,
                    "failed to read subagent usage metadata"
                ),
            }
        }
    }
    None
}

/// pi `metadataUsage`'s default `indexes` (`subagent-cost.ts:127` @ad11b7ab): index `0`, then the
/// unindexed form.
const DEFAULT_METADATA_INDEXES: [Option<usize>; 2] = [Some(0), None];

/// The accumulator behind [`collect_subagent_cost`] — pi's closure state (`:153-176`).
#[derive(Default)]
struct CostCollector {
    children: Vec<SubagentCostChild>,
    child_total: SubagentCostUsage,
    seen: HashSet<String>,
}

impl CostCollector {
    /// pi `addChild` (`subagent-cost.ts:167-182` @ad11b7ab): a zero-usage child is not listed and
    /// answers `false`; a child already listed under the same identity answers `true` without being
    /// counted twice. The identity is `identity` when given (an async run's per-step
    /// `run:<id>:<index>`, so a step does not collide with its run), else `run:<id>`, else
    /// `session:<file>`.
    fn add_child(
        &mut self,
        agent: Option<String>,
        run_id: Option<String>,
        identity: Option<String>,
        usage: Option<SubagentCostUsage>,
        session_file: Option<String>,
    ) -> bool {
        let Some(usage) = usage.filter(SubagentCostUsage::has_value) else {
            return false;
        };
        let identity = identity.or_else(|| {
            run_id
                .as_ref()
                .map(|id| format!("run:{id}"))
                .or_else(|| session_file.as_ref().map(|file| format!("session:{file}")))
        });
        if let Some(identity) = identity
            && !self.seen.insert(identity)
        {
            return true;
        }
        self.child_total.add(&usage);
        self.children.push(SubagentCostChild {
            label: format!(
                "Child {} ({})",
                self.children.len() + 1,
                agent.as_deref().unwrap_or("unknown")
            ),
            agent,
            run_id,
            usage,
            session_file,
        });
        true
    }
}

/// One child result off a details `results` list (pi `subagent-cost.ts:194-196` @ad11b7ab). Its
/// turn count is the usage record's own, else the result's (cyrup keeps `turns` beside `usage`).
fn add_result_child(
    collector: &mut CostCollector,
    run_id: Option<String>,
    result: &serde_json::Value,
) {
    let usage_record = result.get("usage");
    let turns = usage_record
        .and_then(|usage| usage.get("turns"))
        .or_else(|| result.get("turns"));
    collector.add_child(
        string_field(result, "agent"),
        run_id,
        None,
        usage_from_value(usage_record, turns),
        string_field(result, "sessionFile"),
    );
}

/// Collect parent and child usage for one session branch (pi `collectSubagentCost`,
/// `subagent-cost.ts:153-288` @ad11b7ab). Foreground children come from persisted
/// `subagent`/`bg_wait` tool-result details; async workflow children are resolved through their
/// receipts, and other async runs (single, chain, parallel) through their status steps, then
/// artifact metadata.
///
/// `branch` is the root→leaf entry sequence — pi's `ctx.sessionManager.getBranch()`.
///
/// A child result's run id is upstream's `result.runId`, which cyrup's [`crate::exec::SingleResult`]
/// spells `childRunId` (its doc names it the producer of pi's `runId`), so both spellings are read.
/// Likewise its turn count: pi folds `turns` into `result.usage`, cyrup keeps it beside, so a
/// usage record without its own `turns` takes the result's.
pub async fn collect_subagent_cost<'a>(
    branch: impl IntoIterator<Item = &'a Entry>,
    sources: &SubagentCostSources<'_>,
) -> SubagentCostReport {
    let mut parent = SubagentCostUsage::default();
    let mut collector = CostCollector::default();
    // Insertion-ordered, as upstream's `Set`s are (`subagent-cost.ts:162-164` @ad11b7ab).
    let mut workflow_run_ids: Vec<String> = Vec::new();
    let mut async_run_ids: Vec<String> = Vec::new();
    let mut completed_run_ids: Vec<String> = Vec::new();
    let add_unique = |id: &str, ids: &mut Vec<String>| {
        if !ids.iter().any(|seen| seen == id) {
            ids.push(id.to_string());
        }
    };

    for entry in branch {
        if let Some(usage) = parent_usage_from_entry(entry) {
            parent.add(&usage);
        }
        let Some(details) = details_from_session_entry(entry) else {
            continue;
        };
        let results = details
            .get("results")
            .and_then(serde_json::Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let async_id = details
            .get("asyncId")
            .and_then(serde_json::Value::as_str)
            .filter(|id| !id.is_empty());
        let workflow_run_id = details
            .get("runId")
            .and_then(serde_json::Value::as_str)
            .filter(|id| !id.is_empty());
        // pi `subagent-cost.ts:190-193` @ad11b7ab: only an ASYNC workflow persists a receipt
        // (a foreground workflow's child usage is already in its details), and an async launch
        // result has no child results — its usage lands in run artifacts.
        if details.get("mode").and_then(serde_json::Value::as_str) == Some("workflow")
            && let (Some(run_id), Some(_)) = (workflow_run_id, async_id)
        {
            add_unique(run_id, &mut workflow_run_ids);
        } else if let Some(async_id) = async_id
            && results.is_empty()
        {
            add_unique(async_id, &mut async_run_ids);
        }
        for result in results {
            let run_id =
                string_field(result, "runId").or_else(|| string_field(result, "childRunId"));
            add_result_child(&mut collector, run_id, result);
        }
        for (run_id, result) in foreground_workflow_results(details) {
            add_result_child(&mut collector, run_id, result);
        }
        for completion in details
            .get("completions")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            // pi `:197-203`: every completion's run is marked completed, so the async arm below
            // does not count it a second time off its artifacts.
            if let Some(run_id) = completion.get("runId").and_then(serde_json::Value::as_str) {
                if completion.get("mode").and_then(serde_json::Value::as_str) == Some("workflow") {
                    add_unique(run_id, &mut workflow_run_ids);
                }
                add_unique(run_id, &mut completed_run_ids);
            }
            for result in completion
                .get("results")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
            {
                collector.add_child(
                    string_field(result, "agent"),
                    string_field(result, "runId"),
                    None,
                    usage_from_value(result.get("usage"), None),
                    string_field(result, "sessionFile"),
                );
            }
        }
    }

    // pi `:182-194` — the artifacts directories a workflow child's `_meta.json` may be under.
    let mut artifacts_dirs: Vec<PathBuf> = Vec::new();
    let add_artifacts_dir = |cwd: &Path, dirs: &mut Vec<PathBuf>| {
        let dir = crate::artifacts::resolve_artifacts_dir(
            sources.session_file,
            Some(cwd),
            cwd,
            sources.artifact_dir_preference,
        );
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    };
    add_artifacts_dir(sources.cwd, &mut artifacts_dirs);
    add_artifacts_dir(sources.base_cwd, &mut artifacts_dirs);

    let mut unresolved_async_children: u64 = 0;
    for workflow_run_id in &workflow_run_ids {
        // An id that cannot name a run directory is upstream's receipt-read throw (`:250-252`).
        let Some(run) = crate::identity::RunDirName::parse(workflow_run_id) else {
            unresolved_async_children += 1;
            continue;
        };
        let run_dir = run.resolve_in(sources.async_root);
        if let Ok(Some(status)) = crate::background::control::read_status_file(
            &crate::background::RunDir::for_existing(&run_dir).status(),
        )
        .await
            && let Some(cwd) = status.cwd.as_deref()
        {
            add_artifacts_dir(cwd, &mut artifacts_dirs);
        }
        // pi `subagent-cost.ts:250-256` @ad11b7ab: without a receipt, a bg_wait completion cannot
        // prove it reported every child (a stopped workflow may omit one), so a missing or
        // unreadable receipt is COUNTED unresolved. A missing one (a running workflow has no
        // receipt yet; upstream's `ENOENT`, also via `error.cause`) is not logged.
        let receipt = match crate::workflows::read_workflow_receipt(sources.async_root, &run) {
            Ok(receipt) => receipt,
            Err(
                crate::workflows::WorkflowReceiptError::NotFound { .. }
                | crate::workflows::WorkflowReceiptError::MayStillBeActive { .. },
            ) => {
                unresolved_async_children += 1;
                continue;
            }
            Err(error) => {
                unresolved_async_children += 1;
                tracing::warn!(
                    target: "cyrup_ext_subagents::cost",
                    workflow_run_id = %workflow_run_id,
                    %error,
                    "failed to resolve async subagent usage"
                );
                continue;
            }
        };
        let summary_children = receipt
            .workflow_children
            .as_ref()
            .map(|summary| summary.children.as_slice())
            .unwrap_or_default();
        let summary_child = |key: &str| {
            summary_children
                .iter()
                .find(|child| child.child_id.as_str() == key)
        };
        // pi `refsByRunId` (`:207-212`): insertion-ordered, and an agent-less ref is upgraded by a
        // later one that names the agent.
        let mut refs: Vec<(String, Option<String>)> = Vec::new();
        let add_ref = |run_id: Option<&str>, agent: Option<&str>, refs: &mut Vec<_>| {
            let Some(run_id) = run_id.filter(|id| !id.is_empty()) else {
                return;
            };
            match refs
                .iter_mut()
                .find(|(existing, _): &&mut (String, Option<String>)| existing == run_id)
            {
                Some((_, existing_agent)) => {
                    if existing_agent.is_none() && agent.is_some() {
                        *existing_agent = agent.map(str::to_string);
                    }
                }
                None => refs.push((run_id.to_string(), agent.map(str::to_string))),
            }
        };
        for entry in &receipt.entries {
            let summary = summary_child(entry.key.as_str());
            let agent = entry
                .agent
                .as_ref()
                .map(|agent| agent.as_str())
                .or_else(|| summary.and_then(|child| child.agent.as_ref().map(|a| a.as_str())));
            let run_ids: Vec<&str> = if entry.continuation.run_ids.is_empty() {
                entry
                    .resume
                    .latest_run_id()
                    .map(|id| id.as_str())
                    .or_else(|| {
                        summary.and_then(|child| child.run_id.as_ref().map(|id| id.as_str()))
                    })
                    .into_iter()
                    .collect()
            } else {
                entry
                    .continuation
                    .run_ids
                    .iter()
                    .map(String::as_str)
                    .collect()
            };
            for run_id in run_ids {
                add_ref(Some(run_id), agent, &mut refs);
            }
        }
        for child in summary_children {
            if receipt.entry(&child.child_id).is_none() {
                add_ref(
                    child.run_id.as_ref().map(|id| id.as_str()),
                    child.agent.as_ref().map(|agent| agent.as_str()),
                    &mut refs,
                );
            }
        }
        for (run_id, agent) in refs {
            if collector.seen.contains(&format!("run:{run_id}")) {
                continue;
            }
            let Some(agent) = agent else {
                unresolved_async_children += 1;
                continue;
            };
            let usage = metadata_usage(&artifacts_dirs, &run_id, &agent, &DEFAULT_METADATA_INDEXES);
            if usage.is_none() || !collector.add_child(Some(agent), Some(run_id), None, usage, None)
            {
                unresolved_async_children += 1;
            }
        }
    }

    // pi `subagent-cost.ts:259-283` @ad11b7ab: an async single, chain or parallel launch — its
    // usage is resolved through the run's status steps, then each step's artifact metadata.
    for async_run_id in &async_run_ids {
        // A bg_wait completion already reported this run's results; its children carry no child
        // runId.
        if completed_run_ids.contains(async_run_id) {
            continue;
        }
        // An id that cannot name a run directory is upstream's `readStatus` throw (`:279-282`).
        let Some(run) = crate::identity::RunDirName::parse(async_run_id) else {
            unresolved_async_children += 1;
            continue;
        };
        let status_path =
            crate::background::RunDir::for_existing(&run.resolve_in(sources.async_root)).status();
        let status = match crate::background::control::read_status_file(&status_path).await {
            Ok(Some(status)) if !status.steps.is_empty() => status,
            Ok(_) => {
                unresolved_async_children += 1;
                continue;
            }
            Err(error) => {
                unresolved_async_children += 1;
                tracing::warn!(
                    target: "cyrup_ext_subagents::cost",
                    async_run_id = %async_run_id,
                    %error,
                    "failed to resolve async subagent usage"
                );
                continue;
            }
        };
        if let Some(cwd) = status.cwd.as_deref() {
            add_artifacts_dir(cwd, &mut artifacts_dirs);
        }
        let multi_step = status.steps.len() > 1;
        for (index, step) in status.steps.iter().enumerate() {
            // The runner suffixes artifact names with the flat step index only for multi-step
            // runs upstream (`:270-273`); a one-step run reads unindexed, then index `0`. Cyrup's
            // runner always writes `Some(flat_index)` (`background/flat_index.rs`), which the
            // one-step `[None, Some(0)]` order already covers.
            let (indexes, identity) = if multi_step {
                (vec![Some(index)], format!("run:{async_run_id}:{index}"))
            } else {
                (vec![None, Some(0)], format!("run:{async_run_id}"))
            };
            let usage = metadata_usage(&artifacts_dirs, async_run_id, &step.agent, &indexes);
            // Pending and running steps have not finalized their metadata yet.
            let settled = !matches!(
                step.status,
                crate::background::StepState::Pending | crate::background::StepState::Running
            );
            if !collector.add_child(
                Some(step.agent.clone()),
                Some(async_run_id.clone()),
                Some(identity),
                usage,
                None,
            ) && settled
            {
                unresolved_async_children += 1;
            }
        }
    }

    let mut total = SubagentCostUsage::default();
    total.add(&parent);
    total.add(&collector.child_total);
    SubagentCostReport {
        version: SUBAGENT_COST_REPORT_VERSION,
        parent,
        children: collector.children,
        child_total: collector.child_total,
        total,
        unresolved_async_children,
    }
}

/// pi `formatCostUsage` (`subagent-cost.ts:242-249`): `"{label}: ↑{in} ↓{out} ${cost}(...extras)"`,
/// where extras (cache read / cache write / turns) are only appended when non-zero.
fn format_cost_usage(label: &str, usage: &SubagentCostUsage) -> String {
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

/// The `/subagent-cost` text rendering of a collected report (pi `formatSubagentCostReport`,
/// `subagent-cost.ts:252-270`): the Parent line, per-child lines with their optional `Session:`
/// reference, the unresolved-async count when non-zero, a divider, the Children subtotal and the
/// grand Total.
#[must_use]
pub fn format_subagent_cost_report(report: &SubagentCostReport) -> String {
    let mut lines = vec![
        "Subagent cost".to_string(),
        String::new(),
        format_cost_usage("Parent", &report.parent),
    ];
    if report.children.is_empty() {
        lines.push("No subagent child usage found in this session.".to_string());
    } else {
        for child in &report.children {
            lines.push(format_cost_usage(&child.label, &child.usage));
            if let Some(session_file) = &child.session_file {
                lines.push(format!("  Session: {session_file}"));
            }
        }
    }
    if report.unresolved_async_children > 0 {
        lines.push(format!(
            "Async child usage unavailable: {}.",
            report.unresolved_async_children
        ));
    }
    lines.push("────────────────────────────".to_string());
    lines.push(format_cost_usage("Children", &report.child_total));
    lines.push(format_cost_usage("Total", &report.total));
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

        assert_eq!(
            acc.input, expected_input,
            "grandchild input must be included"
        );
        assert_eq!(
            acc.output, expected_output,
            "grandchild output must be included"
        );
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
            native_machine: None,
            runtime_acknowledged_extensions: None,
            watchdog: None,
            process_terminal: None,
            agent: agent.to_string(),
            status: StepState::Complete,
            session_file: None,
            model: Some(ModelId::from(format!("{agent}-model"))),
            attempted_models: vec![ModelId::from(format!("{agent}-model"))],
            usage: usage(input, output, cost_total),
            turns: 0,
            context_overflow: false,
            timeout_recovery: None,
            transcript_path: None,
            transcript_error: None,
            error: None,
            nested_run_ids: Vec::new(),
            started_at: Some(0),
            ended_at: Some(1),
            stop_requested: false,
            stop_requested_at: None,
            stopped: false,
            child_id: None,
            workflow_key: None,
            run_id: None,
            runner: None,
            external_process: None,
            session_name: None,
            label: None,
            interrupted: false,
            output_path_mapping: None,
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
        let grandchild_status = run_status_single_step(grandchild_id.clone(), grandchild_step);
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

        tokio::fs::write(&older, b"{}\n")
            .await
            .expect("write older");
        // Ensure a real, observable mtime gap on filesystems with coarse mtime resolution.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        tokio::fs::write(&newer, b"{}\n")
            .await
            .expect("write newer");

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
    // `/subagent-cost` session-transcript walk (pi `collectSubagentCost` @v0.71.0)
    // ---------------------------------------------------------------------------------------

    /// A `cyrup_core::Usage` (camelCase, nested `cost.total`) JSON value, the shape a real session
    /// stores for both an assistant message's own usage and a subagent child result's usage.
    fn usage_json(
        input: u64,
        output: u64,
        cache_read: u64,
        cache_write: u64,
        cost: f64,
    ) -> serde_json::Value {
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

    /// [`collect_subagent_cost`] with every source rooted under `root`, so the workflow half finds
    /// only what a test put there.
    async fn collect_in<'a>(
        root: &Path,
        branch: impl IntoIterator<Item = &'a Entry>,
    ) -> SubagentCostReport {
        let async_root = root.join("async");
        collect_subagent_cost(
            branch,
            &SubagentCostSources {
                session_file: None,
                cwd: root,
                base_cwd: root,
                artifact_dir_preference: crate::artifacts::ArtifactDirPreference::Project,
                async_root: &async_root,
            },
        )
        .await
    }

    /// The `/subagent-cost` text for `branch`.
    async fn report_of<'a>(branch: impl IntoIterator<Item = &'a Entry>) -> String {
        let dir = tempfile::tempdir().unwrap();
        format_subagent_cost_report(&collect_in(dir.path(), branch).await)
    }

    /// A tool-result entry from any tool.
    fn tool_result_entry(id: &str, tool_name: &str, details: serde_json::Value) -> Entry {
        entry(serde_json::json!({
            "type": "message",
            "id": id,
            "parentId": null,
            "timestamp": "2026-01-01T00:00:00.000Z",
            "message": {
                "role": "toolResult",
                "toolCallId": format!("call-{id}"),
                "toolName": tool_name,
                "content": [{ "type": "text", "text": "done" }],
                "details": details,
                "timestamp": 2,
            },
        }))
    }

    /// pi v0.71.0's collector (`subagent-cost.ts:145-240`) beyond the v0.64 walk: a compaction's
    /// usage is the PARENT's (no turn), a `bg_wait` result's `completions` are children, a child
    /// listed twice under one run id is counted once, and a child's turns come off the result.
    ///
    /// GUT the compaction arm and the parent reads 100; GUT the `bg_wait` name and `reviewer` is
    /// missing; GUT the `run:` identity and `worker` is listed twice; GUT the result-level
    /// `turns` fallback and `worker` reports zero turns.
    #[tokio::test]
    async fn cost_report_counts_compactions_bg_wait_completions_and_dedupes_by_run_id() {
        let compaction = entry(serde_json::json!({
            "type": "compaction",
            "id": "c0000001",
            "parentId": null,
            "timestamp": "2026-01-01T00:00:00.000Z",
            "summary": "s",
            "firstKeptEntryId": "a0000001",
            "tokensBefore": 10,
            "usage": usage_json(10, 5, 0, 0, 0.001),
        }));
        let assistant = assistant_entry("a0000001", None, usage_json(100, 50, 0, 0, 0.01));
        let foreground = tool_result_entry(
            "t0000001",
            "subagent",
            serde_json::json!({
                "mode": "single",
                "results": [{
                    "agent": "worker",
                    "childRunId": "run-a",
                    "turns": 3,
                    "usage": usage_json(40, 20, 0, 0, 0.004),
                }],
            }),
        );
        let waited = tool_result_entry(
            "t0000002",
            "bg_wait",
            serde_json::json!({
                "mode": "management",
                "results": [],
                "completions": [
                    { "runId": "run-a", "results": [{
                        "agent": "worker", "runId": "run-a",
                        "usage": { "input": 40, "output": 20, "cacheRead": 0, "cacheWrite": 0, "cost": 0.004, "turns": 3 },
                    }] },
                    { "runId": "run-b", "results": [{
                        "agent": "reviewer", "runId": "run-b",
                        "usage": { "input": 7, "output": 3, "cacheRead": 0, "cacheWrite": 0, "cost": 0.0007, "turns": 1 },
                    }] },
                ],
            }),
        );

        let dir = tempfile::tempdir().unwrap();
        let report = collect_in(dir.path(), [&compaction, &assistant, &foreground, &waited]).await;

        assert_eq!(report.parent.input, 110, "{report:?}");
        assert_eq!(
            report.parent.turns, 1,
            "a compaction adds no turn: {report:?}"
        );
        let agents: Vec<_> = report
            .children
            .iter()
            .map(|child| child.agent.as_deref().unwrap_or_default())
            .collect();
        assert_eq!(agents, ["worker", "reviewer"], "{report:?}");
        assert_eq!(report.children[0].run_id.as_deref(), Some("run-a"));
        assert_eq!(report.children[0].usage.turns, 3);
        assert_eq!(report.child_total.input, 47);
        assert_eq!(report.total.input, 157);
        assert_eq!(report.unresolved_async_children, 0);

        // The RPC `cost` payload is this struct: upstream's keys, versioned.
        let wire = serde_json::to_value(&report).unwrap();
        assert_eq!(wire["version"], serde_json::json!(1));
        assert_eq!(wire["childTotal"]["input"], serde_json::json!(47));
        assert_eq!(
            wire["children"][1]["label"],
            serde_json::json!("Child 2 (reviewer)")
        );
        assert_eq!(wire["children"][1]["runId"], serde_json::json!("run-b"));
        assert_eq!(wire["unresolvedAsyncChildren"], serde_json::json!(0));
    }

    /// pi `collectSubagentCost`'s workflow half (`:196-236`): a `workflow` result names its run, the
    /// run's receipt names each child's run ids, and a child's usage is read off its artifact
    /// `_meta.json`. A child with no agent, or with no metadata, is COUNTED as unresolved — the
    /// report says its child total is a lower bound rather than silently under-reporting.
    #[tokio::test]
    async fn cost_report_resolves_workflow_children_through_receipt_and_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let run_dir = dir.path().join("async").join("wf-1");
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(
            run_dir.join(crate::workflows::WORKFLOW_RECEIPT_FILE),
            serde_json::to_vec(&serde_json::json!({
                "version": 1,
                "workflowRunId": "wf-1",
                "state": "complete",
                "createdAt": 1,
                "entries": {
                    "a": { "key": "a", "agent": "worker", "continuation": { "runIds": ["r-a1"] },
                           "latestRunId": "r-a1", "resumability": { "state": "resumable" } },
                    "b": { "key": "b", "continuation": { "runIds": ["r-b"] }, "latestRunId": "r-b",
                           "resumability": { "state": "not-resumable", "reason": "no agent" } },
                    "c": { "key": "c", "agent": "reviewer", "continuation": { "runIds": ["r-c"] },
                           "latestRunId": "r-c", "resumability": { "state": "resumable" } },
                },
            }))
            .unwrap(),
        )
        .unwrap();
        let artifacts = crate::artifacts::resolve_artifacts_dir(
            None,
            Some(dir.path()),
            dir.path(),
            crate::artifacts::ArtifactDirPreference::Project,
        );
        std::fs::create_dir_all(&artifacts).unwrap();
        std::fs::write(
            crate::artifacts::artifact_paths(&artifacts, "r-a1", "worker", None).metadata_path,
            serde_json::to_vec(&serde_json::json!({
                "runId": "r-a1",
                "agent": "worker",
                "usage": usage_json(21, 9, 0, 0, 0.002),
            }))
            .unwrap(),
        )
        .unwrap();
        // SUBA-163: only an ASYNC workflow (one carrying `asyncId`) names a receipt to resolve.
        let workflow = tool_result_entry(
            "w0000001",
            "subagent",
            serde_json::json!({ "mode": "workflow", "runId": "wf-1", "asyncId": "wf-1", "results": [] }),
        );

        let report = collect_in(dir.path(), [&workflow]).await;

        assert_eq!(report.children.len(), 1, "{report:?}");
        assert_eq!(report.children[0].run_id.as_deref(), Some("r-a1"));
        assert_eq!(report.children[0].usage.input, 21);
        assert_eq!(
            report.unresolved_async_children, 2,
            "the agent-less `b` and the metadata-less `c`: {report:?}"
        );
        let text = format_subagent_cost_report(&report);
        assert!(text.contains("Async child usage unavailable: 2."), "{text}");
    }

    #[tokio::test]
    async fn cost_report_walks_transcript_and_sums_parent_plus_child_usage() {
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
            assistant_entry(
                "a0000001",
                Some("u0000001"),
                usage_json(200, 100, 0, 0, 0.02),
            ),
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

        let report = report_of(branch.iter()).await;

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
        assert!(
            report.contains("$0.0280"),
            "grand total cost must sum parent+children: {report}"
        );
        // Parent turn count folds in as 1 turn (assistantUsageFromMessage's `turns: 1`).
        assert!(
            report.contains("(1 turn)"),
            "parent turn count must render: {report}"
        );
    }

    #[tokio::test]
    async fn cost_report_empty_transcript_reports_no_child_usage() {
        let report = report_of(std::iter::empty::<&Entry>()).await;
        assert!(report.starts_with("Subagent cost\n"));
        assert!(
            report.contains("No subagent child usage found in this session."),
            "report: {report}"
        );
        assert!(report.contains("Parent: ↑0 ↓0 $0.0000"), "report: {report}");
        assert!(report.contains("Total: ↑0 ↓0 $0.0000"), "report: {report}");
    }

    #[tokio::test]
    async fn cost_report_ignores_non_subagent_tool_results_and_zero_usage_children() {
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

        let report = report_of([&other_tool, &zero_child]).await;
        assert!(
            report.contains("No subagent child usage found in this session."),
            "a non-subagent toolResult and a zero-usage subagent child must both be ignored: {report}"
        );
        assert!(
            !report.contains("nope"),
            "the `read` tool result must not be counted: {report}"
        );
    }

    #[tokio::test]
    async fn cost_report_reads_slash_result_custom_message() {
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

        let report = report_of([&custom]).await;
        assert!(report.contains("Child 1 (scout)"), "report: {report}");
        assert!(report.contains("Children: ↑12 ↓8"), "report: {report}");
    }

    // ---------------------------------------------------------------------------------------
    // SUBA-163 — pi `collectSubagentCost` @ad11b7ab: async launches, `completions`, the
    // async-gated workflow arm, a missing receipt, and foreground workflow children.
    // ---------------------------------------------------------------------------------------

    /// The details a confirmed async launch stores — `extension/executor/paths.rs`'s
    /// `async_launch_details` shape (`{ mode, runId, results: [], asyncId, asyncDir }`), whose
    /// module is private to `extension`.
    fn async_launch_entry(id: &str, mode: &str, run_id: &str, root: &Path) -> Entry {
        tool_result_entry(
            id,
            "subagent",
            serde_json::json!({
                "mode": mode,
                "runId": run_id,
                "results": [],
                "asyncId": run_id,
                "asyncDir": root.join("async").join(run_id),
            }),
        )
    }

    /// Writes `async/<run_id>/status.json` with one step per `(agent, state)`.
    async fn write_async_status(root: &Path, run_id: &str, steps: &[(&str, StepState)]) {
        let run = crate::identity::RunDirName::parse(run_id).expect("a valid run dir name");
        let path =
            crate::background::RunDir::for_existing(&run.resolve_in(&root.join("async"))).status();
        let mut status = RunStatus::queued(
            RunId::from_token(run_id.to_string()),
            RunMode::Chain,
            Some(u32::try_from(steps.len()).unwrap()),
        );
        status.state = RunState::Complete;
        status.steps = steps
            .iter()
            .map(|(agent, state)| {
                let mut step = step_with_usage(agent, 0, 0, 0.0);
                step.status = *state;
                step
            })
            .collect();
        write_atomic_test_json(&path, &status).await;
    }

    /// Writes one artifact `_meta.json` under the project artifacts dir of `root`.
    fn write_meta(root: &Path, run_id: &str, agent: &str, index: Option<usize>, input: u64) {
        let artifacts = crate::artifacts::resolve_artifacts_dir(
            None,
            Some(root),
            root,
            crate::artifacts::ArtifactDirPreference::Project,
        );
        std::fs::create_dir_all(&artifacts).unwrap();
        std::fs::write(
            crate::artifacts::artifact_paths(&artifacts, run_id, agent, index).metadata_path,
            serde_json::to_vec(&serde_json::json!({
                "runId": run_id,
                "agent": agent,
                "usage": usage_json(input, 1, 0, 0, 0.001),
            }))
            .unwrap(),
        )
        .unwrap();
    }

    /// pi `subagent-cost.ts:193,259-278` @ad11b7ab: an async single launch's result has an
    /// `asyncId` and no `results`, so its usage is read off the run's status steps and artifact
    /// metadata. Cyrup's runner always writes the indexed `_0` form for a one-step run.
    ///
    /// Before SUBA-163 the collector tracked workflow run ids only, and this reported no children.
    #[tokio::test]
    async fn an_async_single_launch_contributes_its_child_usage() {
        let dir = tempfile::tempdir().unwrap();
        write_async_status(dir.path(), "as-1", &[("worker", StepState::Complete)]).await;
        write_meta(dir.path(), "as-1", "worker", Some(0), 30);
        let launch = async_launch_entry("t0000001", "single", "as-1", dir.path());

        let report = collect_in(dir.path(), [&launch]).await;

        assert_eq!(report.children.len(), 1, "{report:?}");
        assert_eq!(report.children[0].agent.as_deref(), Some("worker"));
        assert_eq!(report.children[0].run_id.as_deref(), Some("as-1"));
        assert_eq!(report.child_total.input, 30);
        assert_eq!(report.unresolved_async_children, 0, "{report:?}");
        // The RPC `cost` payload is the same struct.
        let wire = serde_json::to_value(&report).unwrap();
        assert_eq!(wire["childTotal"]["input"], serde_json::json!(30));
    }

    /// pi `:268-278`: a multi-step async run (chain or parallel) reads each step's own indexed
    /// metadata under a per-step identity, so two steps of one run do not collapse into one. A
    /// pending step is not yet unresolved; a settled step with no metadata is.
    #[tokio::test]
    async fn an_async_chain_and_parallel_launch_each_contribute_every_steps_usage() {
        for mode in ["chain", "parallel"] {
            let dir = tempfile::tempdir().unwrap();
            let run_id = format!("{mode}-1");
            write_async_status(
                dir.path(),
                &run_id,
                &[
                    ("scout", StepState::Complete),
                    ("worker", StepState::Complete),
                    ("reviewer", StepState::Failed),
                    ("tester", StepState::Pending),
                ],
            )
            .await;
            write_meta(dir.path(), &run_id, "scout", Some(0), 10);
            write_meta(dir.path(), &run_id, "worker", Some(1), 20);
            // Index 0 is the scout's slot: the worker must not be read from it.
            write_meta(dir.path(), &run_id, "worker", Some(0), 999);
            let launch = async_launch_entry("t0000001", mode, &run_id, dir.path());

            let report = collect_in(dir.path(), [&launch]).await;

            let agents: Vec<_> = report
                .children
                .iter()
                .map(|child| child.agent.as_deref().unwrap_or_default())
                .collect();
            assert_eq!(agents, ["scout", "worker"], "{mode}: {report:?}");
            assert_eq!(report.child_total.input, 30, "{mode}: {report:?}");
            assert_eq!(
                report.unresolved_async_children, 1,
                "{mode}: the settled metadata-less reviewer, not the pending tester: {report:?}"
            );
        }
    }

    /// pi `:197-199,261`: a `bg_wait` completion for an async run marks it completed, so its usage
    /// is counted once, off the completion, and not again off the run's artifacts.
    #[tokio::test]
    async fn a_bg_wait_completion_does_not_double_count_an_async_launch() {
        let dir = tempfile::tempdir().unwrap();
        write_async_status(
            dir.path(),
            "ap-1",
            &[
                ("scout", StepState::Complete),
                ("worker", StepState::Complete),
            ],
        )
        .await;
        write_meta(dir.path(), "ap-1", "scout", Some(0), 10);
        write_meta(dir.path(), "ap-1", "worker", Some(1), 20);
        let launch = async_launch_entry("t0000001", "parallel", "ap-1", dir.path());
        let waited = tool_result_entry(
            "t0000002",
            crate::extension::wait_tool::WAIT_TOOL_NAME,
            serde_json::json!({
                "mode": "management",
                "results": [],
                "completions": [{ "runId": "ap-1", "mode": "parallel", "results": [
                    { "agent": "scout", "usage": { "input": 10, "output": 1, "cost": 0.001, "turns": 1 } },
                    { "agent": "worker", "usage": { "input": 20, "output": 1, "cost": 0.001, "turns": 1 } },
                ] }],
            }),
        );

        let report = collect_in(dir.path(), [&launch, &waited]).await;

        assert_eq!(report.children.len(), 2, "{report:?}");
        assert_eq!(report.child_total.input, 30, "{report:?}");
        assert_eq!(report.unresolved_async_children, 0, "{report:?}");
    }

    /// pi `:191` @ad11b7ab: a workflow id joins the receipt set only with an `asyncId`. A
    /// foreground workflow's details name its run but have no receipt to resolve, so nothing is
    /// read for it and nothing is counted unresolved — even when a receipt for that id exists.
    #[tokio::test]
    async fn a_foreground_workflow_run_id_is_not_resolved_through_a_receipt() {
        let dir = tempfile::tempdir().unwrap();
        let run_dir = dir.path().join("async").join("wf-fg");
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(
            run_dir.join(crate::workflows::WORKFLOW_RECEIPT_FILE),
            serde_json::to_vec(&serde_json::json!({
                "version": 1,
                "workflowRunId": "wf-fg",
                "state": "complete",
                "createdAt": 1,
                "entries": {
                    "a": { "key": "a", "agent": "worker", "continuation": { "runIds": ["r-fg"] },
                           "latestRunId": "r-fg", "resumability": { "state": "resumable" } },
                },
            }))
            .unwrap(),
        )
        .unwrap();
        write_meta(dir.path(), "r-fg", "worker", None, 40);
        let foreground = tool_result_entry(
            "w0000001",
            "subagent",
            serde_json::json!({ "mode": "workflow", "runId": "wf-fg", "results": [] }),
        );

        let report = collect_in(dir.path(), [&foreground]).await;

        assert!(report.children.is_empty(), "{report:?}");
        assert_eq!(report.unresolved_async_children, 0, "{report:?}");
    }

    /// pi `:250-256` @ad11b7ab (#2615): an async workflow with no receipt yet is COUNTED as
    /// unresolved, so the report says its child total is a lower bound.
    #[tokio::test]
    async fn an_async_workflow_without_a_receipt_is_listed_as_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        let running = tool_result_entry(
            "w0000001",
            "subagent",
            serde_json::json!({ "mode": "workflow", "runId": "wf-2", "asyncId": "wf-2", "results": [] }),
        );

        let report = collect_in(dir.path(), [&running]).await;

        assert_eq!(report.unresolved_async_children, 1, "{report:?}");
        let text = format_subagent_cost_report(&report);
        assert!(text.contains("Async child usage unavailable: 1."), "{text}");
    }

    /// pi `workflowDetailsResults` (`src/runs/foreground/subagent-executor.ts:4758-4764`
    /// @ad11b7ab): a foreground workflow's child usage is counted, every round of a resumed child
    /// under its own run id (#2612), and a round without one under the child's.
    #[tokio::test]
    async fn a_foreground_workflow_counts_every_childs_usage() {
        let dir = tempfile::tempdir().unwrap();
        let workflow = tool_result_entry(
            "w0000001",
            "subagent",
            serde_json::json!({
                "mode": "workflow",
                "workflowRunId": "wf-fg",
                "children": [
                    { "key": "a", "ok": true, "runId": "r-a", "output": "",
                      "results": [{ "agent": "scout", "turns": 2, "usage": usage_json(11, 1, 0, 0, 0.001) }] },
                    { "key": "b", "ok": true, "runId": "r-b2", "output": "",
                      "results": [
                          { "agent": "worker", "runId": "r-b1", "usage": usage_json(5, 1, 0, 0, 0.001) },
                          { "agent": "worker", "runId": "r-b2", "usage": usage_json(7, 1, 0, 0, 0.001) },
                      ] },
                ],
            }),
        );
        // The failure arm's shape: no `workflowRunId` when settlement itself failed, same `children`.
        let failed = tool_result_entry(
            "w0000002",
            "subagent",
            serde_json::json!({
                "mode": "workflow",
                "children": [{ "key": "c", "ok": false, "runId": "r-c", "output": "",
                               "results": [{ "agent": "tester", "usage": usage_json(3, 1, 0, 0, 0.001) }] }],
            }),
        );

        let report = collect_in(dir.path(), [&workflow, &failed]).await;

        let runs: Vec<_> = report
            .children
            .iter()
            .map(|child| child.run_id.as_deref().unwrap_or_default())
            .collect();
        assert_eq!(runs, ["r-a", "r-b1", "r-b2", "r-c"], "{report:?}");
        assert_eq!(report.children[0].usage.turns, 2);
        assert_eq!(report.child_total.input, 26, "{report:?}");
        assert_eq!(report.unresolved_async_children, 0, "{report:?}");
    }
}
