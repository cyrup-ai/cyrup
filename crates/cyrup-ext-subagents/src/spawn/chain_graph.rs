//! The linear chain/workflow-graph walker (func-SA §5.3 R-SA-052/053; arch-SA §2.2/§4.2/§6.4).
//!
//! # This is explicitly NOT a general DAG scheduler
//!
//! [`RunnerStep`] is a three-shape discriminated union (`SingleStep | ParallelGroup |
//! DynamicGroup`) and [`ChainGraph`] is nothing more than `Vec<RunnerStep>`, walked strictly in
//! list order (R-SA-052). There is no node-id graph, no forward/backward edge table, and no
//! topological sort anywhere in this file — building any of that would be non-conformant with
//! R-SA-052's explicit text ("never as a general DAG with arbitrary forward/backward edges").
//! [`walk_chain`] is a synchronous fold over the list that, for each element in order, either
//! runs one `SingleStep` inline or delegates the whole group to [`crate::spawn::parallel::
//! run_bounded`] before moving on to the next list element — "walk the list, await each element
//! (or bounded-fan-out group) to completion before moving to the next" is the entire algorithm
//! (arch-SA §6.4).
//!
//! Cross-step data dependencies are expressible **only** through named-output references
//! (`{outputs.name}`) resolved against strictly earlier steps' validated `structured_output`
//! results (R-SA-053) — never through a graph edge, never against unstructured prose (this
//! restates R-SA-030 at the chain-graph level). [`OutputRegistry`] is this file's small,
//! append-only accumulator for that resolution: each completed step may register a named output,
//! and [`OutputRegistry::resolve`]/[`OutputRegistry::resolve_pointer`] can only ever see outputs
//! already registered by a *strictly earlier* step — there is no way to reference a later or
//! sibling step's output, by construction (the registry is built up incrementally as
//! [`walk_chain`] proceeds, so a step being evaluated literally cannot observe anything not yet
//! inserted).
//!
//! # Delegation to `spawn/parallel.rs` (and `spawn/worktree.rs`)
//!
//! `ParallelGroup`/`DynamicGroup` fan-out execution (bounded `Semaphore`-gated worker pools over
//! real child OS processes, R-SA-049/050/051) is owned by [`crate::spawn::parallel::run_bounded`],
//! a sibling module in this crate. This module never re-implements that fan-out logic — every
//! group step in a [`ChainGraph`] is dispatched by calling straight into `run_bounded` with a
//! worker closure that dispatches one [`SingleStepSpec`] via the same [`SingleStepExecutor`] seam
//! [`RunnerStep::SingleStep`] itself uses, exactly matching `parallel.rs`'s and `worktree.rs`'s
//! own module-header commitment ("the chain/workflow driver is expected to call `run_bounded`
//! with a worker closure ..."; "once `spawn::chain_graph` lands, its fan-out driver is expected to
//! call straight into `setup_worktree_group` ... no change to this module's own logic is
//! anticipated"). A `worktree: true` group additionally routes through
//! [`crate::spawn::worktree::setup_worktree_group`] first (R-SA-060-064) to assign each fanned-out
//! task its own dedicated worktree `cwd` before `run_bounded` dispatches it.
//!
//! `SingleStepExecutor` (the actual "spawn a real child OS process for this one agent
//! invocation" primitive) is a narrow seam this module depends on rather than a concrete
//! `exec::run_sync` call, because `exec/mod.rs` (func-SA §5.2's foreground executor, a later
//! phase of this crate's build-out) does not exist yet as of this file — see that trait's own doc
//! comment for the exact hand-off contract a landing `exec/` module is expected to satisfy.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use cyrup_core::{CancelToken, ModelId};
use serde_json::Value;

use crate::discovery::types::{AgentReadScope, OutputMode};
use crate::error::SubagentError;
use crate::fork_context::ContextMode;
use crate::spawn::parallel::{FanOutResult, GlobalConcurrencyLimit, run_bounded};

// -------------------------------------------------------------------------------------------
// RunnerStep: the three-shape discriminated union (func-SA §4.2)
// -------------------------------------------------------------------------------------------

/// One agent invocation's full parameter surface (func-SA §4.2 `SingleStep`). Every field here
/// maps to one concrete piece of what the spawn boundary (`spawn/mod.rs`) and the foreground
/// executor (a later phase's `exec/mod.rs`) need to build a `ChildSpawnSpec` and interpret its
/// result — this type is deliberately data-only; it carries no execution logic itself.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SingleStepSpec {
    /// The fully-qualified agent name to invoke (matches [`crate::discovery::types::
    /// AgentDefinition::name`] via exact string equality only).
    pub agent: String,
    /// The task prompt. May contain `{outputs.name}` references resolved against strictly
    /// earlier steps' `structured_output` results (R-SA-053) before this step is dispatched —
    /// resolution itself is [`OutputRegistry::resolve`]'s job, invoked by the walker just before
    /// dispatch, never baked into this struct's own (de)serialization.
    pub task: String,
    /// Working directory override for this step. In a `worktree: true` [`ParallelGroupSpec`],
    /// an explicit per-task `cwd` here MUST be rejected before any child is spawned (R-SA-062) —
    /// enforced by [`crate::spawn::worktree::reject_task_level_cwd_overrides`], called by
    /// [`walk_chain`] itself for any `worktree: true` group before delegating to `run_bounded`.
    pub cwd: Option<PathBuf>,
    /// Explicit model override for this step, taking precedence over the agent's own configured
    /// model when present (func-SA §4.3 `RunOptions::model_override`, R-SA-041's inherit
    /// sentinel — `None` here means "no step-level override", not "use some global default").
    pub model: Option<ModelId>,
    /// Per-step tool allowlist override. `None` = defer to the agent's own resolved tools;
    /// `Some(vec![])` = no tools; `Some(populated)` = exactly this allowlist (mirrors
    /// `AgentDefinition::tools`' tri-state shape, func-SA §4.1).
    pub tools: Option<Vec<crate::discovery::types::ToolRef>>,
    /// Per-step extension allowlist override, same tri-state shape as `tools` above.
    pub extensions: Option<Vec<String>>,
    /// Explicit session-file path to hand the child (e.g. a fork-context branch path already
    /// resolved by [`crate::fork_context::ForkContextResolver`] before this step is dispatched).
    pub session_file: Option<PathBuf>,
    /// Per-step override of the recursion-depth ceiling (R-SA-056's tightening-only rule applies
    /// exactly as it does to `AgentDefinition::max_subagent_depth` — this field can only lower
    /// the inherited ceiling further, never raise it; that enforcement lives in `spawn/depth.rs`,
    /// not this type).
    pub max_depth_override: Option<u32>,
    /// JSON Schema the child's `structured_output` MUST validate against, when present
    /// (R-SA-030). `None` means this step has no structured-output requirement.
    pub structured_output_schema: Option<Value>,
    /// Named-output key this step's result is registered under in the chain-wide
    /// [`OutputRegistry`], if any (func-SA §4.2 `as`). Absent means this step's result is not
    /// referenceable by any later step's `{outputs.name}` reference. (This is pi's `as` field — the
    /// registry KEY — never the output FILE path, which is [`Self::output_path`] below.)
    pub output: Option<String>,
    /// The output FILE path this step's final output is written to (pi's `output` field,
    /// `chain-execution.ts:254`/`subagent-runner.ts:872`). A relative path is resolved against the
    /// step's effective cwd (the run/chain cwd) at dispatch time by
    /// [`crate::background::runner_main::ExecSingleStepExecutor::run_single`], which then hands the
    /// resolved absolute path to [`crate::exec::run_sync`] via [`crate::exec::RunOptions::output_path`]
    /// so the stat-snapshot file-output handoff (`exec/output.rs`) runs and the saved-output
    /// reference message is emitted. `None` means this step writes no output file.
    pub output_path: Option<String>,
    /// Where/how this step's final output is written (func-SA §4.2 `outputMode`).
    pub output_mode: Option<OutputMode>,
    /// Pre-declared read-context paths for this step (func-SA §4.2 `reads`).
    pub reads: Option<Vec<PathBuf>>,
    /// Explicit acceptance-contract override for this step (func-SA §4.2 `acceptance`); `None`
    /// defers to the agent's own default / heuristic inference (R-SA-023).
    ///
    /// This is the RAW wire value pi carries on a step (`ChainStep["acceptance"]`, pi
    /// `chain-execution.ts:400` `acceptance: task.acceptance` / `:1335` `acceptance:
    /// seqStep.acceptance`) — a level string (`"checked"`), the `false` shorthand, or a full
    /// `AcceptanceConfig` object (`{ level, verify: [{ command }], … }`) — never a pre-lowered
    /// contract. [`crate::background::runner_main::ExecSingleStepExecutor::run_single`] lowers it
    /// onto a real [`crate::exec::acceptance::AcceptanceContract`] via
    /// [`crate::exec::acceptance::lower_acceptance_input`] at dispatch, the same single lowering the
    /// SINGLE-mode `acceptance` tool param uses.
    ///
    /// SUBA-N04: this was `Option<String>`, which silently discarded every object/`false` form on
    /// the way in and was then hard-dropped to `None` on the way out, so a step declaring an
    /// acceptance contract ran completely UNVERIFIED and reported success on the accepted-run path.
    pub acceptance: Option<Value>,
    /// Fork-vs-fresh session context for this step. `None` defers to the agent's own
    /// `default_context` (func-SA §4.1).
    pub context: Option<ContextMode>,
    /// Per-step override of discovery scope (func-SA §4.3 `RunOptions::agent_scope`).
    pub agent_scope: Option<AgentReadScope>,
}

/// A static-width fan-out over a fixed list of [`SingleStepSpec`] (func-SA §4.2 `ParallelGroup`).
/// Execution (bounded concurrency, worktree isolation, fail-fast semantics) is delegated to
/// [`crate::spawn::parallel::run_bounded`] (and, when `worktree` is set,
/// [`crate::spawn::worktree::setup_worktree_group`] first) by [`walk_chain`] — this type is pure
/// data.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParallelGroupSpec {
    /// The fixed list of steps to fan out over. Width is static (known at chain-parse time) —
    /// contrast with [`DynamicGroupSpec`], whose width resolves only at chain-walk time.
    pub steps: Vec<SingleStepSpec>,
    /// Local worker-pool concurrency ceiling for this group (R-SA-049): at most this many
    /// children of this group are ever concurrently live, further capped by the run-wide
    /// `global_concurrency_limit` (R-SA-050) that [`ChainRunContext::global_limit`] carries.
    pub concurrency: u32,
    /// Cooperative fail-fast (R-SA-066): once one child in this group fails, prevent new work
    /// from starting, but MUST NOT kill already-dispatched, still-running siblings. Enforced by
    /// `spawn/parallel.rs::run_bounded`, not this type.
    pub fail_fast: bool,
    /// Whether each concurrently-spawned child in this group gets its own dedicated git-worktree
    /// cwd (R-SA-060/061), rather than a shared cwd. Enforced by `spawn/worktree.rs` +
    /// `spawn/parallel.rs`, not this type.
    pub worktree: bool,
}

/// A runtime-width fan-out whose item count resolves from a prior step's validated structured
/// output (func-SA §4.2 `DynamicGroup`). `expand` is a JSON-Pointer-equivalent path resolved
/// against a strictly earlier step's `structured_output` (R-SA-053) — never against unstructured
/// prose — and `template` is instantiated once per resolved array element to produce the
/// concrete `SingleStepSpec` list that gets fanned out over via the exact same `run_bounded`
/// delegation [`ParallelGroupSpec`] uses.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicGroupSpec {
    /// `{outputs.name}`-qualified, JSON-Pointer-equivalent path identifying which strictly
    /// earlier step's `structured_output` supplies the array to expand over, and where within
    /// that structured output the array itself lives (R-SA-053). Resolved by
    /// [`OutputRegistry::resolve_pointer`], never by this type itself.
    pub expand: String,
    /// The per-item step template (pi `DynamicParallelStep::parallel`). One concrete
    /// [`SingleStepSpec`] is materialized per element of the array [`DynamicGroupSpec::expand`]
    /// resolves to, with its `task` (and only its `task`) re-substituted per item via
    /// [`crate::spawn::dynamic_fanout::resolve_item_template`] against the [`Self::item`] name
    /// (C16) before the flat `{outputs.name}` pass — so every fanned-out child gets its OWN task
    /// string, not a shared one.
    pub template: Box<SingleStepSpec>,
    /// Named-output key the *collected* (fanned-in) array of per-item results is registered
    /// under in the chain-wide [`OutputRegistry`] (pi `collect.as`). Unlike a
    /// [`ParallelGroupSpec`], the registered value is the ordered collect-record array
    /// ([`crate::spawn::dynamic_fanout::DynamicCollectedResult`]), not the raw child-structured-
    /// output array — matching pi's `outputs[collect.as] = { structured: collected }`.
    pub collect: String,
    /// Local worker-pool concurrency ceiling for the expanded group, identical in meaning to
    /// [`ParallelGroupSpec::concurrency`].
    pub concurrency: u32,
    /// The template variable name each `{item}`/`{item.path}` reference binds to (pi
    /// `expand.item`); `None` means the pi default `"item"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
    /// Optional `expand.key` JSON Pointer resolved against each array element to derive that
    /// element's dedup key (pi `expand.key`); `None` means "use the element's index as the key".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// The `expand.maxItems` cap (pi `expand.maxItems`). The EFFECTIVE cap is this value, or —
    /// when absent — [`ChainRunContext::dynamic_fanout_max_items`] (pi's
    /// `config.chain.dynamicFanout.maxItems`); if neither is set, materialization errors, exactly
    /// as pi does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_items: Option<u32>,
    /// Behavior when the resolved source array is empty (pi `expand.onEmpty`): [`OnEmpty::Skip`]
    /// registers an empty collect array and continues; [`OnEmpty::Fail`] fails the step.
    #[serde(default, skip_serializing_if = "OnEmpty::is_default")]
    pub on_empty: OnEmpty,
    /// Optional JSON Schema the *aggregate* collect-record array MUST validate against (pi
    /// `collect.outputSchema`), checked by
    /// [`crate::spawn::dynamic_fanout::validate_dynamic_collection`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collect_schema: Option<Value>,
}

/// Behavior when a [`DynamicGroupSpec`]'s resolved source array is empty (pi `expand.onEmpty`,
/// `dynamic-fanout.ts:245`). Serializes as the pi string values `"skip"`/`"fail"`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnEmpty {
    /// Register an empty collect array and continue the chain (pi default).
    #[default]
    Skip,
    /// Fail the dynamic step with `"... source array is empty."` (pi `onEmpty: "fail"`).
    Fail,
}

impl OnEmpty {
    /// Whether this is the default ([`OnEmpty::Skip`]) — used to keep the serialized form minimal.
    #[must_use]
    fn is_default(&self) -> bool {
        matches!(self, OnEmpty::Skip)
    }
}

/// The parameters for attaching a brand-new chain's first step to another, already-launched
/// async/background run's result (func-SA §5.4 R-SA-097; pi `chain-root-attachment.ts`'s
/// `ImportedAsyncRoot`). A [`RunnerStep::ImportAsyncRoot`] carrying this spec is NOT dispatched by
/// spawning a child process at all — the background runner (`background/runner_main.rs::run_inner`)
/// intercepts it and instead POLLS the target run's `status.json`/terminal `ResultFile` via
/// [`crate::background::control::wait_for_imported_async_root`] until it goes terminal, then
/// synthesizes the imported outcome as this chain's first step's result. This type is pure data,
/// carrying only what is needed to (a) rebuild the target run's [`crate::background::RunPaths`]
/// (`run_id`/`async_root`/`results_dir`) and (b) pick the right child within a multi-child target
/// result (`index`), plus the display `agent` shown for the synthesized step before its poll
/// resolves and the optional named `output` key the imported result is registered under for later
/// `{outputs.name}` references (pi's `outputName`/`as`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportAsyncRootSpec {
    /// The target run's id token — combined with `async_root`/`results_dir` to rebuild the target's
    /// [`crate::background::RunPaths`]. Kept a plain `String` (not the `background::RunId` newtype)
    /// so this `spawn`-module type introduces no dependency on the `background` module (the runner,
    /// which lives in `background`, is the one place that lifts it back into a `RunId` before
    /// polling).
    pub run_id: String,
    /// The target run's absolute `AsyncRoot` (holding `<async_root>/<run_id>/status.json`).
    pub async_root: PathBuf,
    /// The target run's absolute `ResultsDir` (holding `<results_dir>/<run_id>.json`).
    pub results_dir: PathBuf,
    /// Which child within the target run's terminal `ResultFile.results` this attachment imports
    /// (`0` for a single-mode target; the specific slot for a parallel/chain target).
    pub index: usize,
    /// The agent name displayed for the synthesized step before (and, as a fallback, after) its
    /// poll resolves — the imported result's own agent name takes precedence once available.
    pub agent: String,
    /// Optional named-output key the imported result is registered under in the chain-wide
    /// [`OutputRegistry`] (pi's `outputName`/`as`) — `None` means the imported root's output is not
    /// referenceable by any later `{outputs.name}` reference.
    pub output: Option<String>,
}

/// The discriminated union `SingleStep | ParallelGroup | DynamicGroup | ImportAsyncRoot`
/// (func-SA §4.2, extended by R-SA-097's root-attachment step). Tagged JSON so chain files
/// (`.chain.json`/`.chain.md`) and the one-shot runner-config hand-off file (arch-SA §4.3) can
/// (de)serialize it directly.
///
/// This enum, and [`ChainGraph`] below, are the SOLE representation of a chain/workflow: there is
/// no separate node-id/edge-table graph type anywhere in this crate (R-SA-052).
///
/// `clippy::large_enum_variant` is deliberately allowed here rather than boxing
/// [`SingleStepSpec`]: `RunnerStep::SingleStep` is by far this enum's most common, hottest-path
/// variant (every chain step is a `SingleStep`, directly or as a `ParallelGroup`/`DynamicGroup`'s
/// per-child template), so boxing it would trade a large-`match`-arm clippy nit for an extra heap
/// allocation/indirection on the overwhelmingly common path, with `RunnerStep` values themselves
/// never stored in a hot, size-sensitive collection anywhere in this crate (a [`ChainGraph`] is a
/// small, one-off-per-chain `Vec`, not a per-turn/per-tool-call hot structure) — the standard
/// tradeoff this lint exists to flag does not apply here.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[allow(clippy::large_enum_variant)]
pub enum RunnerStep {
    /// One agent invocation, dispatched inline by [`walk_chain`] (no fan-out machinery
    /// involved).
    SingleStep(SingleStepSpec),
    /// A static-width fan-out, delegated to [`crate::spawn::parallel::run_bounded`] before
    /// [`walk_chain`] proceeds to the next list element.
    ParallelGroup(ParallelGroupSpec),
    /// A runtime-width fan-out, likewise delegated to `run_bounded` after its `expand` source
    /// array is resolved against the [`OutputRegistry`].
    DynamicGroup(DynamicGroupSpec),
    /// A chain-root attachment (R-SA-097): synthesize this chain's first step from another
    /// already-launched async run's result by POLLING (never spawning). Dispatched by the
    /// background runner's own pre-walk interception
    /// (`background/runner_main.rs::run_inner` -> [`crate::background::control::
    /// wait_for_imported_async_root`]), NOT by [`walk_chain`] — which has no `background` dependency
    /// and so treats this variant defensively (see [`walk_chain`]'s own arm).
    ImportAsyncRoot(ImportAsyncRootSpec),
}

/// `ChainGraph` is nothing more than an ordered list of [`RunnerStep`]s, walked strictly in order
/// (R-SA-052). This type alias exists purely for readability at call sites — it carries no
/// additional structure, no node ids, and no edges beyond "comes before"/"comes after" in the
/// `Vec`'s own order.
pub type ChainGraph = Vec<RunnerStep>;

// -------------------------------------------------------------------------------------------
// OutputRegistry: named-output cross-step data dependencies (R-SA-053)
// -------------------------------------------------------------------------------------------

/// One named chain output, mirroring pi's `ChainOutputMapEntry` (`chain-outputs.ts:100-107`): a
/// step registers both a display `text` (what a `{outputs.name}` reference substitutes to) and,
/// when the producing step returned validated structured JSON, the `structured` value a downstream
/// `DynamicGroup.expand` pointer walks (R-SA-053; `dynamic-fanout.ts:221`). A plain-text step has
/// `structured == None`, so a `{outputs.name}` reference to it still resolves (to its text) but a
/// dynamic-fanout `expand` against it fails with the "no structured output" diagnostic pi raises.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainOutputEntry {
    /// The text a `{outputs.name}` reference substitutes to (pi `entry.text`): for a structured
    /// output, the compact JSON encoding (`JSON.stringify`, `chain-outputs.ts:96-97`); for a plain
    /// text output, the step's final output text verbatim.
    pub text: String,
    /// The validated structured-output value, when the producing step returned one — the source a
    /// `DynamicGroup.expand` JSON pointer resolves against (pi `entry.structured`). `None` for a
    /// plain-text output.
    pub structured: Option<Value>,
}

/// The append-only accumulator of named step outputs [`walk_chain`] builds up as it proceeds
/// through a [`ChainGraph`] (R-SA-053). A step being evaluated can only ever resolve references
/// against outputs already registered by strictly earlier steps — there is no API on this type
/// that can observe an output not yet inserted, so "strictly earlier only" is structural, not
/// merely a convention this type's callers must remember to uphold.
///
/// It also carries the rolling [`Self::previous`] output text that a step's `{previous}` placeholder
/// (and, absent an explicit `{previous}`, `build_chain_instructions`'s "Previous step output" suffix)
/// resolves to (pi's `prev` local, `chain-execution.ts:608/1049/1219`). Threading `previous` through
/// the registry — rather than a walker-local — is what lets the background hop-2 runner (which drives
/// the identical chain one `walk_chain(one_step)` call at a time over a single shared registry,
/// `background/runner_main.rs:855`) get the same step-to-step `{previous}` piping the foreground
/// full-graph walk gets, with no second code path.
#[derive(Debug, Default, Clone)]
pub struct OutputRegistry {
    outputs: BTreeMap<String, ChainOutputEntry>,
    previous: String,
}

impl OutputRegistry {
    /// A fresh, empty registry — the starting state at the top of one chain run.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `name`'s validated structured-output value once its producing step has
    /// completed. Called by [`walk_chain`] immediately after a [`SingleStepSpec`] with a
    /// `Some(output)` name (or a [`DynamicGroupSpec`]'s `collect` name) finishes — never before,
    /// which is exactly what makes "strictly earlier steps only" hold for every subsequent
    /// [`OutputRegistry::resolve`]/[`OutputRegistry::resolve_pointer`] call made while walking
    /// the remainder of the chain.
    ///
    /// A later registration under an already-used `name` overwrites the earlier one — chain
    /// authors are responsible for using distinct output names across steps if they need both
    /// values later; this registry does not itself detect or reject a name collision (that
    /// diagnostic, if ever added, belongs to chain-file validation in a later phase, not this
    /// runtime accumulator).
    ///
    /// Registers a **structured** output: the entry's [`ChainOutputEntry::text`] becomes the value's
    /// compact JSON encoding (pi's `compactStructuredText` = `JSON.stringify`, `chain-outputs.ts:96`)
    /// so a `{outputs.name}` reference substitutes that JSON text, while [`ChainOutputEntry::structured`]
    /// retains the raw value for a later `DynamicGroup.expand` pointer to walk.
    pub fn register(&mut self, name: impl Into<String>, value: Value) {
        let text = compact_structured_text(&value);
        self.outputs.insert(
            name.into(),
            ChainOutputEntry {
                text,
                structured: Some(value),
            },
        );
    }

    /// Register a **plain-text** output (a step that produced final text but no validated structured
    /// JSON) under `name` (C11; pi `outputEntryFromResult` with `structuredOutput === undefined`,
    /// `chain-outputs.ts:100-107`). A later `{outputs.name}` reference substitutes `text` verbatim;
    /// a `DynamicGroup.expand` against it fails with the "no structured output" diagnostic, matching
    /// pi's `resolveDynamicFanoutItems` `requires structured output` guard (`dynamic-fanout.ts:221`).
    pub fn register_text(&mut self, name: impl Into<String>, text: impl Into<String>) {
        self.outputs.insert(
            name.into(),
            ChainOutputEntry {
                text: text.into(),
                structured: None,
            },
        );
    }

    /// The rolling previous-step output text a `{previous}` placeholder resolves to (pi's `prev`,
    /// `chain-execution.ts:1049`). Empty before the first step (and after any step that produced no
    /// text output).
    #[must_use]
    pub fn previous(&self) -> &str {
        &self.previous
    }

    /// Record `text` as the previous-step output for the next step's `{previous}` resolution
    /// (pi's `prev = getSingleResultOutput(r)`, `chain-execution.ts:1219`). [`walk_chain`] calls
    /// this only after a step **succeeds** — a failed step both halts the chain (C9) and leaves
    /// `previous` untouched, exactly as pi never reaches its `prev = …` assignment on a nonzero
    /// exit (the failed-summary return at `chain-execution.ts:1188-1198` precedes it).
    pub fn set_previous(&mut self, text: impl Into<String>) {
        self.previous = text.into();
    }

    /// Fetch a strictly-earlier step's whole registered output by name, or `None` if no step has
    /// registered that name yet (either because no step ever will, or — from the walker's own
    /// point of view while mid-chain — because the producing step has not run yet, which by
    /// construction cannot happen for a well-formed `{outputs.name}` reference resolved at its
    /// own step's dispatch time, since chain authoring only ever references earlier steps).
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&ChainOutputEntry> {
        self.outputs.get(name)
    }

    /// Resolve every `{outputs.name}` reference embedded in `template` against this registry's
    /// currently-registered outputs, substituting each with the referenced output's
    /// [`ChainOutputEntry::text`] (R-SA-053; a faithful port of pi's `resolveOutputReferences`,
    /// `chain-outputs.ts:85-94`). This is NOT the mechanical, error-swallowing substitution the
    /// prior implementation performed: matching pi, an occurrence whose name is malformed, or which
    /// names an output no strictly-earlier step registered, is a hard error (C11) — a chain that
    /// references a nonexistent output must fail rather than silently ship an unresolved
    /// `{outputs.…}` placeholder into a child's prompt.
    ///
    /// An unclosed `{outputs.` with no `}` before end-of-string is left verbatim (pi's regex
    /// `/\{outputs\.([^}]*)\}/g` simply does not match it), never an error.
    ///
    /// # Errors
    ///
    /// Returns [`SubagentError::ChainOutputInvalid`] carrying pi's exact message when a reference's
    /// name violates `/^[A-Za-z_][A-Za-z0-9_]*$/` (`Invalid chain output reference '…'.`) or names
    /// an unregistered output (`Unknown chain output reference '{outputs.name}'.`).
    pub fn resolve(&self, template: &str) -> Result<String, SubagentError> {
        let mut result = String::with_capacity(template.len());
        let mut rest = template;
        while let Some(start) = rest.find("{outputs.") {
            result.push_str(&rest[..start]);
            let after_prefix = &rest[start + "{outputs.".len()..];
            let Some(end) = after_prefix.find('}') else {
                // No closing brace at all for the remainder of the string: emit the rest
                // verbatim and stop scanning (pi's regex would not match an unclosed reference,
                // so it is left in place, not treated as an error).
                result.push_str(&rest[start..]);
                rest = "";
                break;
            };
            let name = &after_prefix[..end];
            if !is_safe_output_name(name) {
                return Err(SubagentError::ChainOutputInvalid(format!(
                    "Invalid chain output reference '{{outputs.{name}}}'. Use {{outputs.name}} \
                     with /^[A-Za-z_][A-Za-z0-9_]*$/ names."
                )));
            }
            match self.outputs.get(name) {
                Some(entry) => result.push_str(&entry.text),
                None => {
                    return Err(SubagentError::ChainOutputInvalid(format!(
                        "Unknown chain output reference '{{outputs.{name}}}'."
                    )));
                }
            }
            rest = &after_prefix[end + 1..];
        }
        result.push_str(rest);
        Ok(result)
    }

    /// Resolve a [`DynamicGroupSpec::expand`] source pointer into the concrete JSON array it
    /// names (R-SA-053: "a JSON-Pointer-equivalent path against a prior step's validated
    /// structured output only — never against unstructured prose").
    ///
    /// Accepted `pointer` shape: `outputs.<name>` optionally followed by a `/`-delimited
    /// RFC-6901-style JSON-Pointer suffix identifying where, within that step's registered
    /// output, the array itself lives (e.g. `outputs.plan/items` reaches the `items` field of the
    /// JSON object registered under the name `plan`; `outputs.plan` alone means the whole
    /// registered value is expected to already be the array). A `~0`/`~1` escaped pointer segment
    /// is unescaped per RFC 6901 before being used as an object key, matching ordinary JSON
    /// Pointer semantics.
    ///
    /// # Errors
    ///
    /// Returns [`SubagentError::StructuredOutputInvalid`] if: `pointer` does not start with the
    /// mandatory `outputs.` prefix; the named output was never registered (i.e. does not resolve
    /// to a strictly earlier step's structured output at all); the pointer path walks through a
    /// JSON value that is not an object/array at some segment; or the final resolved value is not
    /// a JSON array.
    pub fn resolve_pointer(&self, pointer: &str) -> Result<&[Value], SubagentError> {
        let Some(rest) = pointer.strip_prefix("outputs.") else {
            return Err(SubagentError::StructuredOutputInvalid(format!(
                "dynamic-group expand pointer {pointer:?} must start with \"outputs.\""
            )));
        };
        let (name, path) = match rest.split_once('/') {
            Some((name, path)) => (name, Some(path)),
            None => (rest, None),
        };

        let entry = self.outputs.get(name).ok_or_else(|| {
            SubagentError::StructuredOutputInvalid(format!(
                "expand pointer references output \"{name}\", which is not a strictly-earlier \
                 step's registered structured output"
            ))
        })?;
        // pi's `resolveDynamicFanoutItems` rejects a source output that carries no `structured`
        // value (a plain-text step's output) before touching the pointer path
        // (`dynamic-fanout.ts:221`): a `{outputs.name}` text reference resolves, but expanding over
        // it does not.
        let mut current = entry.structured.as_ref().ok_or_else(|| {
            SubagentError::StructuredOutputInvalid(format!(
                "expand pointer references output \"{name}\", which produced plain text with no \
                 validated structured output to expand over"
            ))
        })?;

        if let Some(path) = path {
            for raw_segment in path.split('/') {
                let segment = unescape_json_pointer_segment(raw_segment);
                current = match current {
                    Value::Object(map) => map.get(&segment).ok_or_else(|| {
                        SubagentError::StructuredOutputInvalid(format!(
                            "expand pointer segment {segment:?} not found in output \"{name}\""
                        ))
                    })?,
                    Value::Array(items) => segment
                        .parse::<usize>()
                        .ok()
                        .and_then(|i| items.get(i))
                        .ok_or_else(|| {
                            SubagentError::StructuredOutputInvalid(format!(
                                "expand pointer segment {segment:?} is not a valid array index \
                                 into output \"{name}\""
                            ))
                        })?,
                    _ => {
                        return Err(SubagentError::StructuredOutputInvalid(format!(
                            "expand pointer walks through a non-object/array value in output \
                             \"{name}\" at segment {segment:?}"
                        )));
                    }
                };
            }
        }

        match current {
            Value::Array(items) => Ok(items.as_slice()),
            _ => Err(SubagentError::StructuredOutputInvalid(format!(
                "expand pointer {pointer:?} resolved to a non-array value; \
                 DynamicGroup.expand must resolve to a validated structured-output array"
            ))),
        }
    }
}

/// Unescape one RFC-6901 JSON-Pointer segment (`~1` -> `/`, then `~0` -> `~`, in that order —
/// reversing the escape order the spec mandates for encoding).
fn unescape_json_pointer_segment(segment: &str) -> String {
    segment.replace("~1", "/").replace("~0", "~")
}

/// pi's `SAFE_OUTPUT_NAME_PATTERN` (`/^[A-Za-z_][A-Za-z0-9_]*$/`, `chain-outputs.ts:7`): a
/// non-empty identifier starting with an ASCII letter or underscore, then ASCII alphanumerics/
/// underscores. Kept regex-free (this crate carries no `regex` dependency) but character-for-
/// character equivalent to pi's pattern.
fn is_safe_output_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// pi's `compactStructuredText` (`chain-outputs.ts:96-97`): the compact JSON encoding
/// (`JSON.stringify`) of a structured output, used as the `{outputs.name}` substitution text for a
/// structured step. `serde_json::to_string` emits the same separator-free compact form; a value
/// that fails to serialize (not reachable for a `serde_json::Value`, which always serializes) falls
/// back to its `Display`, never a panic (workspace no-`unwrap`/`expect` rule).
fn compact_structured_text(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
}

// -------------------------------------------------------------------------------------------
// Template substitution (C10/C11): {outputs.name}/{task}/{previous}/{chain_dir} + chain
// instructions prefix/suffix (chain-execution.ts:1039-1052, settings.ts:312-357)
// -------------------------------------------------------------------------------------------

/// Resolve a chain-relative file path against `chain_dir` the way pi's `resolveChainPath`
/// (`settings.ts:299-300`) does: an absolute path is used verbatim; a relative path is joined onto
/// `chain_dir`. Rendered lossily to a `String` for embedding in the child's prompt text.
fn resolve_chain_path(file: &Path, chain_dir: &Path) -> String {
    if file.is_absolute() {
        file.to_string_lossy().into_owned()
    } else {
        chain_dir.join(file).to_string_lossy().into_owned()
    }
}

/// Build the prefix/suffix a chain step's task is wrapped with, a faithful port of pi's
/// `buildChainInstructions` (`settings.ts:312-357`):
///
/// - Each declared `reads` file becomes a leading `[Read from: <resolved paths>]` line (prepended so
///   it overrides any hardcoded filename in the task text).
/// - A declared output file becomes a leading `[Write to: <resolved path>]` line.
/// - When `previous_summary` is `Some(non-blank)`, a trailing `Previous step output:\n<trimmed>`
///   block is appended under a `\n\n---\n` rule. The caller passes `Some(prev)` only when the task
///   template does **not** already contain an explicit `{previous}` placeholder (pi's
///   `templateHasPrevious ? undefined : prev`, `chain-execution.ts:1044`) — an explicit `{previous}`
///   is substituted inline instead, and the previous output is not also appended.
///
/// pi's progress-file suffix (`Create/Update progress at: …`) is intentionally not emitted here:
/// [`SingleStepSpec`] carries no `progress` flag (progress-file orchestration is a separate,
/// unported concern), so this port covers the reads/output/previous-summary components that drive
/// step-to-step data flow, which is what C10 scopes.
fn build_chain_instructions(
    reads: Option<&[PathBuf]>,
    output_path: Option<&str>,
    chain_dir: &Path,
    previous_summary: Option<&str>,
) -> (String, String) {
    let mut prefix_parts: Vec<String> = Vec::new();
    let mut suffix_parts: Vec<String> = Vec::new();

    if let Some(reads) = reads
        && !reads.is_empty()
    {
        let files: Vec<String> = reads
            .iter()
            .map(|f| resolve_chain_path(f, chain_dir))
            .collect();
        prefix_parts.push(format!("[Read from: {}]", files.join(", ")));
    }

    if let Some(output) = output_path {
        let resolved = resolve_chain_path(Path::new(output), chain_dir);
        prefix_parts.push(format!("[Write to: {resolved}]"));
    }

    if let Some(prev) = previous_summary {
        let trimmed = prev.trim();
        if !trimmed.is_empty() {
            suffix_parts.push(format!("Previous step output:\n{trimmed}"));
        }
    }

    let prefix = if prefix_parts.is_empty() {
        String::new()
    } else {
        format!("{}\n\n", prefix_parts.join("\n"))
    };
    let suffix = if suffix_parts.is_empty() {
        String::new()
    } else {
        format!("\n\n---\n{}", suffix_parts.join("\n"))
    };
    (prefix, suffix)
}

/// Resolve one step's `task` template into the concrete prompt text handed to the child, applying —
/// in pi's exact order (`chain-execution.ts:1047-1052`) — `{outputs.name}` reference substitution
/// (failing on unknown/malformed names, C11), then `{task}` -> the chain's original task, then
/// `{previous}` -> the previous step's output, then `{chain_dir}` -> the chain working directory,
/// and finally wrapping the result in [`build_chain_instructions`]'s reads/output/previous-summary
/// prefix & suffix.
///
/// # Errors
///
/// Propagates [`OutputRegistry::resolve`]'s [`SubagentError::ChainOutputInvalid`] for an
/// unknown/malformed `{outputs.name}` reference.
fn resolve_step_task(
    template: &str,
    registry: &OutputRegistry,
    ctx: &ChainRunContext,
    spec: &SingleStepSpec,
) -> Result<String, SubagentError> {
    // Capture whether the AUTHOR wrote an explicit {previous} before any substitution — this
    // decides whether build_chain_instructions also appends the previous output as a suffix.
    let template_has_previous = template.contains("{previous}");

    // 1. {outputs.name} -> the referenced output's text (hard error on unknown/malformed).
    let mut task = registry.resolve(template)?;
    // 2. {task} -> the chain's original top-level task.
    task = task.replace("{task}", &ctx.original_task);
    // 3. {previous} -> the previous step's output text.
    let prev = registry.previous();
    task = task.replace("{previous}", prev);
    // 4. {chain_dir} -> the chain working directory.
    let chain_dir = ctx.chain_dir.as_deref().unwrap_or(ctx.cwd.as_path());
    let chain_dir_str = chain_dir.to_string_lossy();
    task = task.replace("{chain_dir}", &chain_dir_str);

    // 5. Wrap in the reads/output/previous-summary instructions.
    let previous_summary = if template_has_previous { None } else { Some(prev) };
    let (prefix, suffix) = build_chain_instructions(
        spec.reads.as_deref(),
        spec.output_path.as_deref(),
        chain_dir,
        previous_summary,
    );
    Ok(format!("{prefix}{task}{suffix}"))
}

/// Register a completed step's result under `name` in `registry` (C11), choosing the structured vs
/// plain-text entry shape exactly as pi's `outputEntryFromResult` does (`chain-outputs.ts:100-107`):
/// a result carrying a validated `structured_output` registers as structured (JSON-encoded text +
/// retained structured value); otherwise it registers as plain text from the step's final output.
/// A `None` name is a no-op (the step declared no `as` key).
fn register_single_output(registry: &mut OutputRegistry, name: Option<&str>, result: &StepResult) {
    let Some(name) = name else { return };
    match &result.structured_output {
        Some(value) => registry.register(name.to_string(), value.clone()),
        None => registry.register_text(
            name.to_string(),
            result.final_output.clone().unwrap_or_default(),
        ),
    }
}

// -------------------------------------------------------------------------------------------
// Upfront output-binding validation (R-SA-053, pi `chain-outputs.ts::validateChainOutputBindings`,
// called once at the very top of `executeChain`, `chain-execution.ts:499-510`, BEFORE any step is
// dispatched)
// -------------------------------------------------------------------------------------------

/// The named output(s) `step` registers once it completes (pi
/// `chain-outputs.ts::outputNamesForStep`): a sequential step's `as`, EVERY parallel task's own
/// `as` (a static group can register more than one name, one per task), or a dynamic group's single
/// `collect.as`. Empty/absent names are omitted, mirroring pi's own empty-string filter.
fn output_names_for_runner_step(step: &RunnerStep) -> Vec<String> {
    match step {
        RunnerStep::SingleStep(spec) => spec
            .output
            .as_deref()
            .filter(|name| !name.is_empty())
            .map(|name| vec![name.to_string()])
            .unwrap_or_default(),
        RunnerStep::ParallelGroup(group) => group
            .steps
            .iter()
            .filter_map(|task| task.output.as_deref())
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect(),
        RunnerStep::DynamicGroup(spec) => {
            if spec.collect.is_empty() {
                Vec::new()
            } else {
                vec![spec.collect.clone()]
            }
        }
        RunnerStep::ImportAsyncRoot(spec) => spec
            .output
            .as_deref()
            .filter(|name| !name.is_empty())
            .map(|name| vec![name.to_string()])
            .unwrap_or_default(),
    }
}

/// The task-template string(s) `step`'s `{outputs.name}` references are scanned in (pi
/// `chain-outputs.ts::taskTemplatesForStep`): every parallel task's own `task`, a dynamic group
/// template's `task`, or a sequential step's `task`.
fn task_templates_for_runner_step(step: &RunnerStep) -> Vec<String> {
    match step {
        RunnerStep::SingleStep(spec) => vec![spec.task.clone()],
        RunnerStep::ParallelGroup(group) => {
            group.steps.iter().map(|task| task.task.clone()).collect()
        }
        RunnerStep::DynamicGroup(spec) => vec![spec.template.task.clone()],
        RunnerStep::ImportAsyncRoot(_) => Vec::new(),
    }
}

/// Extract every `{outputs.<name>}` reference from a template (pi's `\{outputs\.([^}]*)\}`),
/// returning each `(raw_match, name)` pair — a direct port of `chain-outputs.ts::extractOutputRefs`.
fn extract_output_refs(template: &str) -> Vec<(String, String)> {
    const PREFIX: &str = "{outputs.";
    let mut refs = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find(PREFIX) {
        let after = rest.get(start + PREFIX.len()..).unwrap_or("");
        let Some(end) = after.find('}') else {
            break;
        };
        let name = after.get(..end).unwrap_or("");
        refs.push((format!("{{outputs.{name}}}"), name.to_string()));
        rest = after.get(end + 1..).unwrap_or("");
    }
    refs
}

/// The `expand.from.output` source-output name a [`DynamicGroupSpec::expand`] pointer
/// (`"outputs.<name><path>"`, see [`OutputRegistry::resolve_pointer`]) was built from — the SAME
/// parse [`OutputRegistry::resolve_pointer`] applies (strip the `"outputs."` prefix, split at the
/// first `/`), used here only to name-check the source against strictly-earlier registered outputs
/// before any step runs.
fn dynamic_expand_source_output(expand: &str) -> &str {
    let rest = expand.strip_prefix("outputs.").unwrap_or(expand);
    rest.split_once('/').map_or(rest, |(name, _)| name)
}

/// Faithful port of `chain-outputs.ts::validateChainOutputBindings` (pi's empty-context call from
/// `chain-execution.ts:499-510`, run ONCE at the very top of `executeChain` before any step is
/// dispatched), operating directly over the already-typed [`RunnerStep`] graph — the structural
/// analogue of [`crate::discovery::chains`]'s identically-named raw-JSON port that saved-chain-file
/// parsing already applies. Checks, in chain order:
///
/// - Every named output (`as`/`collect.as`) is a safe identifier and, chain-wide, UNIQUE — a
///   second step registering an already-used name errors here instead of silently overwriting the
///   earlier registration in [`OutputRegistry`] at run time.
/// - A [`RunnerStep::DynamicGroup`]'s `expand` source names a STRICTLY EARLIER step's output.
/// - Every `{outputs.name}` reference in a step's own task template(s) is a safe identifier naming
///   a STRICTLY EARLIER step's output — before this validation, an unknown/malformed reference was
///   only caught when [`walk_chain`] actually reached that step (`resolve_step_task`), after every
///   earlier step had already run (and spent real tokens/spawned real children).
///
/// # Errors
///
/// Returns pi's exact user/LLM-facing message text (`Duplicate chain output name '…'…`, `Invalid
/// chain output name/reference '…'…`, `Unknown chain output reference '…'…`, `Dynamic chain step N
/// references unknown output '…'…`) the first time any check fails; the whole graph passes silently
/// (`Ok(())`) when every step's bindings are well-formed.
pub fn validate_runner_step_output_bindings(graph: &[RunnerStep]) -> Result<(), String> {
    let mut available: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for (step_index, step) in graph.iter().enumerate() {
        let display = step_index + 1;

        if let RunnerStep::DynamicGroup(spec) = step {
            let source_output = dynamic_expand_source_output(&spec.expand);
            if !available.contains(source_output) {
                return Err(format!(
                    "Dynamic chain step {display} references unknown output '{source_output}'. \
                     Named outputs are only available after producing step/group completes."
                ));
            }
        }

        for name in output_names_for_runner_step(step) {
            if !is_safe_output_name(&name) {
                return Err(format!(
                    "Invalid chain output name '{name}' at step {display}. Use \
                     /^[A-Za-z_][A-Za-z0-9_]*$/."
                ));
            }
            if seen.contains(&name) {
                return Err(format!(
                    "Duplicate chain output name '{name}'. Each as name must be unique."
                ));
            }
            seen.insert(name);
        }

        for template in task_templates_for_runner_step(step) {
            for (raw_reference, name) in extract_output_refs(&template) {
                if !is_safe_output_name(&name) {
                    return Err(format!(
                        "Invalid chain output reference '{raw_reference}' at step {display}. Use \
                         {{outputs.name}} with /^[A-Za-z_][A-Za-z0-9_]*$/ names."
                    ));
                }
                if !available.contains(&name) {
                    return Err(format!(
                        "Unknown chain output reference '{raw_reference}' at step {display}. \
                         Named outputs are only available after producing step/group completes."
                    ));
                }
            }
        }

        for name in output_names_for_runner_step(step) {
            available.insert(name);
        }
    }
    Ok(())
}

/// Aggregate a completed group's per-child text outputs into the `{previous}` text a following
/// sequential step sees (pi's `prev = aggregateParallelOutputs(taskResults)`,
/// `chain-execution.ts:773` / `parallel-utils.ts:166-192`). [`walk_chain`] calls this only for a
/// fully-successful group, so every child's status line is empty and the body is just its output —
/// this port emits pi's default `=== Parallel Task N (agent) ===` header + output body, joined by a
/// blank line. `agents[i]` names each child's agent (position-aligned with `children`).
fn aggregate_group_previous(children: &[Option<StepResult>], agents: &[String]) -> String {
    children
        .iter()
        .enumerate()
        .map(|(i, child)| {
            let agent = agents.get(i).map_or("", String::as_str);
            let output = child
                .as_ref()
                .and_then(|r| r.final_output.as_deref())
                .unwrap_or("");
            format!("=== Parallel Task {} ({}) ===\n{}", i + 1, agent, output)
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

// -------------------------------------------------------------------------------------------
// StepResult and the SingleStepExecutor seam
// -------------------------------------------------------------------------------------------

/// The result of running exactly one [`SingleStepSpec`] to completion (arch-SA §6.4's "pre-sized
/// `Vec<Option<StepResult>>` indexed by original position" restated at the per-step granularity).
///
/// This is a narrow, chain-graph-local view — NOT `exec::SingleResult` (a later phase's fuller
/// per-run record, func-SA §4.3), which this file has no dependency on. `structured_output` is
/// carried here specifically because [`walk_chain`] needs it to populate the [`OutputRegistry`]
/// for later steps' `{outputs.name}` references (R-SA-053) without reaching into a fuller result
/// type this module does not own.
#[derive(Clone, Debug, PartialEq)]
pub struct StepResult {
    /// `true` iff this step completed without a hard failure.
    pub success: bool,
    /// The step's validated structured output, when produced. `None` for a plain-text-only
    /// result.
    pub structured_output: Option<Value>,
    /// The step's plain-text final output, when produced (func-SA §4.3 `SingleResult::
    /// final_output`'s narrow chain-graph-local analogue).
    pub final_output: Option<String>,
    /// A human-readable failure summary, present only when `success` is `false`.
    pub error: Option<String>,
    /// A soft interrupt (`RunOptions.interrupt`) fired on this step's child mid-flight (R-SA-084) —
    /// the child was signalled and torn down, and the run should treat this step as the pause point
    /// rather than a clean completion. Distinct from `success`: an interrupted step reports
    /// `success: true` (pi's paused-success, exit 0, cleared error) but carries `interrupted: true`
    /// so the background runner (`background/runner_main.rs`) can mark it `Paused` and end the run
    /// `Paused`, never `Complete`. `false` for every non-interrupted step.
    #[doc(alias = "paused")]
    pub interrupted: bool,
    /// SUBA-N05 — the live-control events this step's child raised
    /// ([`crate::exec::SingleResult::control_events`], pi `result.controlEvents`,
    /// `runs/foreground/execution.ts:1260` @v0.34.0).
    ///
    /// Carried here for the same reason `structured_output` is: something one layer OUT needs it
    /// and there is no other channel. The detached hop-2 runner collapses every step into a
    /// `SingleResult` through
    /// [`crate::background::runner_main::step_result_to_single_result`] before writing the terminal
    /// `ResultFile`, so without this field an async run's control events were raised, counted, and
    /// then discarded at this boundary — the orchestrator saw an empty `controlEvents` no matter
    /// what `control` the run was launched with. Upstream's async runner does not lose them either:
    /// it appends each one to the run's control-event log for the parent tracker to replay
    /// (`runs/background/subagent-runner.ts:2270-2280` → `async-job-tracker.ts:138-166`).
    ///
    /// This is a plain, serializable data vector — it does NOT make this module depend on
    /// [`crate::exec::SingleResult`], the fuller per-run record the type doc above rules out.
    ///
    /// Empty for every step whose control config was disabled, whose `notifyOn` excluded both
    /// classes, or that simply never tripped a threshold.
    pub control_events: Vec<crate::exec::control::ControlEvent>,
}

impl StepResult {
    /// Construct a successful result.
    #[must_use]
    pub fn success(final_output: Option<String>, structured_output: Option<Value>) -> Self {
        Self {
            success: true,
            structured_output,
            final_output,
            error: None,
            interrupted: false,
            control_events: Vec::new(),
        }
    }

    /// Construct a failed result.
    #[must_use]
    pub fn failure(error: impl Into<String>) -> Self {
        Self {
            success: false,
            structured_output: None,
            final_output: None,
            error: Some(error.into()),
            interrupted: false,
            control_events: Vec::new(),
        }
    }
}

/// One [`RunnerStep::ParallelGroup`]/[`RunnerStep::DynamicGroup`]'s fully collapsed outcome,
/// combining the aggregate [`StepResult`] the chain's own [`OutputRegistry`] cares about with the
/// full, position-ordered per-child detail [`crate::spawn::parallel::run_bounded`] returned
/// (R-SA-051) — retained verbatim so a caller needing individual-child detail (not just the
/// aggregate) never loses it.
#[derive(Clone, Debug, PartialEq)]
pub struct GroupStepResult {
    /// The collapsed aggregate (`success` iff every child succeeded; `structured_output` is the
    /// JSON array of each child's own `structured_output`, `Value::Null` for a child with none).
    pub aggregate: StepResult,
    /// One entry per fanned-out child, in original (pre-sized, position-indexed) order
    /// (R-SA-051) — `None` for a task [`crate::spawn::parallel::run_bounded`] never dispatched
    /// (fail-fast skip or cancellation).
    pub children: Vec<Option<StepResult>>,
}

/// Everything [`walk_chain`] needs to dispatch one [`SingleStepSpec`] inline, threaded straight
/// through from the caller (foreground executor or background runner) without this module
/// inventing its own copy of run-wide state.
///
/// Deliberately entirely `Clone` (every field is either `Clone` itself or wrapped so it is) —
/// [`crate::spawn::parallel::run_bounded`]'s `Worker` closure bound requires `'static`, so
/// [`dispatch_group`] clones this context once per fan-out call and moves the clone into the
/// worker closure, rather than borrowing across that boundary (this crate is
/// `#![forbid(unsafe_code)]`, so no raw-pointer/lifetime-erasure trick is used anywhere to work
/// around that bound — an owned, cheaply-`Clone`-able context is the plain, safe alternative).
#[derive(Clone)]
pub struct ChainRunContext {
    /// The base working directory steps without their own `cwd` override run in. Also the
    /// shared repository cwd a `worktree: true` [`ParallelGroupSpec`] validates via
    /// [`crate::spawn::worktree::check_clean_working_tree`] before any worktree is created.
    pub cwd: PathBuf,
    /// The chain-wide deadline (R-SA-035: monotonically shrinking, computed once, passed through
    /// unmodified to every step — never reset per step).
    pub deadline_at: Option<Instant>,
    /// The chain-wide nominal timeout budget in milliseconds (pi `chain-execution.ts:606`:
    /// `deadlineAt = params.deadlineAt ?? Date.now() + timeoutMs`, with `timeoutMs` ALSO passed
    /// verbatim to every `runSync` call alongside `deadlineAt`). Distinct from [`Self::deadline_at`]
    /// (the actual wall-clock instant raced against): this is only the nominal figure
    /// [`crate::exec::format_timeout_message`] renders into a step's timed-out error text — the
    /// SAME `timeout_ms` every step in the chain reports, never a shrinking "time remaining" value.
    /// `None` when the chain run carries no timeout at all.
    pub timeout_ms: Option<u64>,
    /// The run-wide cancellation signal, raced against every dispatched step/group.
    pub cancel: CancelToken,
    /// The run-wide global concurrency ceiling (R-SA-050), shared across every
    /// [`RunnerStep::ParallelGroup`]/[`RunnerStep::DynamicGroup`] within this one chain run —
    /// constructed once per chain run and threaded through every [`run_bounded`] call this
    /// walker makes, so no group independently re-derives it.
    pub global_limit: GlobalConcurrencyLimit,
    /// Optional worktree-group configuration; required (an error is returned) only for a
    /// [`ParallelGroupSpec`] with `worktree: true`. `None` is fine for any chain with no
    /// worktree-isolated group.
    pub worktree_base_dir: Option<PathBuf>,
    /// The chain's overall original task text (pi `originalTask`, `chain-execution.ts:1048`), the
    /// value every step's `{task}` placeholder resolves to. Empty when the run carries no distinct
    /// top-level task (the substitution then yields the empty string, matching pi when `params.task`
    /// is empty).
    pub original_task: String,
    /// The chain working directory (pi `chainDir`, `chain-execution.ts:1050`) that `{chain_dir}`
    /// resolves to and that [`build_chain_instructions`]'s `[Read from: …]`/`[Write to: …]` prefix
    /// paths are resolved against. `None` falls back to [`Self::cwd`] (a chain with no dedicated
    /// scratch directory writes/reads relative to its own cwd).
    pub chain_dir: Option<PathBuf>,
    /// The run-wide dynamic-fanout item cap (pi `config.chain.dynamicFanout.maxItems`), used as the
    /// fallback when a [`RunnerStep::DynamicGroup`]'s own [`DynamicGroupSpec::max_items`] is absent
    /// (R-SA-053 / C16). `None` means "no config cap" — a dynamic step that also omits
    /// `expand.maxItems` then fails materialization, exactly as pi does when neither is set.
    /// (Wiring the real config value here is the Tier-6 `chain.dynamicFanout.maxItems` config-key
    /// task; today's foreground/background callers pass `None`.)
    pub dynamic_fanout_max_items: Option<u32>,
}

/// The single dispatch primitive [`walk_chain`] uses to run exactly one [`SingleStepSpec`]
/// inline (both directly, for [`RunnerStep::SingleStep`], and as the `run_bounded` worker
/// closure's own inner call for every fanned-out child of a group).
///
/// A later phase's foreground executor (`exec/mod.rs`) supplies the real implementation
/// (spawning a real child OS process via [`crate::spawn::SpawnedChild`], per func-SA §1.1's
/// mandated mechanism); this module depends on it only through this narrow async-trait seam so
/// `chain_graph.rs` itself never needs a direct dependency on `exec/`'s not-yet-landed types.
/// [`walk_chain`]/[`dispatch_group`] always receive this behind an `Arc` (never a bare
/// reference) so it can be cheaply cloned into `run_bounded`'s `'static` worker closures without
/// `unsafe` code.
#[async_trait::async_trait]
pub trait SingleStepExecutor: Send + Sync {
    /// Run `step` (with `task` already resolved against the [`OutputRegistry`], R-SA-053, and
    /// `cwd` already overridden to a dedicated worktree path when running inside a
    /// `worktree: true` group) to completion and return its [`StepResult`]. MUST spawn a genuine
    /// OS subprocess per func-SA §1.1 — never an in-process turn loop — but that mechanism is
    /// entirely this trait implementation's own responsibility, not something [`walk_chain`]
    /// inspects or enforces itself.
    async fn run_single(
        &self,
        step: &SingleStepSpec,
        resolved_task: &str,
        ctx: &ChainRunContext,
    ) -> Result<StepResult, SubagentError>;
}

// -------------------------------------------------------------------------------------------
// walk_chain: the linear walker itself (R-SA-052/053)
// -------------------------------------------------------------------------------------------

/// Walk `graph` strictly in order (R-SA-052), dispatching each [`RunnerStep`] to completion
/// before proceeding to the next list element:
///
/// - [`RunnerStep::SingleStep`] is resolved against `registry` (R-SA-053's `{outputs.name}`
///   substitution) and dispatched inline via `single`.
/// - [`RunnerStep::ParallelGroup`] has every one of its `steps`' `task` fields resolved against
///   `registry` the same way. If `worktree` is set, [`crate::spawn::worktree::
///   setup_worktree_group`] runs first (R-SA-060-064) and each resolved step's `cwd` is
///   overridden to its assigned worktree path; the whole (possibly worktree-adjusted) step list
///   is then delegated to [`crate::spawn::parallel::run_bounded`] in one call
///   (R-SA-049/050/051/066 — this function does not itself bound concurrency or preserve
///   ordering; that is entirely `run_bounded`'s contract).
/// - [`RunnerStep::DynamicGroup`] first resolves `expand` via [`OutputRegistry::resolve_pointer`]
///   against a strictly earlier step's validated structured output (R-SA-053/030) — a resolution
///   failure here aborts the whole chain walk immediately, before `run_bounded` is ever called
///   for this step, since a `DynamicGroup` with an unresolvable source array has no well-defined
///   width to fan out over at all. On success, `template` is instantiated once per resolved array
///   element (see this function's doc note below on template instantiation) and the resulting
///   concrete step list is delegated to `run_bounded` exactly like a [`RunnerStep::ParallelGroup`]
///   (a `DynamicGroup` never opts into worktree isolation itself — func-SA §4.2 gives it no
///   `worktree` field).
///
/// After every step, if it produced a named output (`SingleStepSpec::output` /
/// `DynamicGroupSpec::collect`), that output is registered into `registry` before the next list
/// element is walked — this is precisely what makes "strictly earlier steps only" hold: a step's
/// own output becomes visible to `{outputs.name}` resolution starting with the very next element,
/// never before.
///
/// Returns one [`StepResult`] per [`RunnerStep`] in `graph`, in the same order — this function's
/// own return value is itself just another pre-sized, position-indexed `Vec` (mirroring
/// R-SA-051's ordering discipline at the whole-chain granularity, even though within-group
/// ordering is `run_bounded`'s own separate responsibility, preserved verbatim in
/// [`GroupStepResult::children`] when a caller needs it — see `group_results` below).
///
/// `group_results` is populated with one entry per [`RunnerStep::ParallelGroup`]/
/// [`RunnerStep::DynamicGroup`] encountered (in chain order, NOT indexed by overall step
/// position), retaining `run_bounded`'s full per-child detail for callers that need more than the
/// collapsed [`StepResult`] this function's own return `Vec` carries for that step.
///
/// # Errors
///
/// Returns `Err` (aborting the remainder of the walk immediately, before any later step is
/// dispatched) if: a `DynamicGroup.expand` pointer fails to resolve (R-SA-053/030); a
/// `worktree: true` group's setup fails (R-SA-060-064, surfaced via
/// [`crate::spawn::worktree::setup_worktree_group`]); or a `worktree: true` group is encountered
/// with no `worktree_base_dir` configured on `ctx`.
///
/// # Template instantiation for `DynamicGroup` (C16, pi `materializeDynamicParallelStep`)
///
/// Each resolved source-array element is materialized into its OWN distinct [`SingleStepSpec`], so
/// every fanned-out child gets a per-item task string — never the shared, identical task the
/// pre-C16 walker produced. For every element, the `DynamicGroup`'s `template.task` is first run
/// through [`crate::spawn::dynamic_fanout::resolve_item_template`] to bind `{item}` / `{item.path}`
/// (the item name defaulting to `"item"`, or `expand.item`) against that element's own fields, and
/// only THEN through [`resolve_step_task`]'s flat `{outputs.name}` / `{task}` / `{previous}` /
/// `{chain_dir}` + chain-instruction pass — matching pi's materialize-then-dispatch order
/// (`dynamic-fanout.ts:250-259`). The raw `template.task` is validated up front against
/// [`crate::spawn::dynamic_fanout::assert_no_unresolved_item_references`] (pi
/// `validateDynamicStepShape`, run before source/maxItems/duplicate resolution), the effective
/// `maxItems` cap and duplicate-key / colliding-id detection come from
/// [`crate::spawn::dynamic_fanout::resolve_dynamic_fanout_items`], `onEmpty` decides the empty-array
/// behavior, and after the group runs the per-child results are folded into the ordered
/// collect-record array ([`crate::spawn::dynamic_fanout::collect_dynamic_results`]) registered under
/// `collect.as` (optionally schema-validated) — the full `dynamic-fanout.ts:137-240` surface.
pub async fn walk_chain(
    graph: &ChainGraph,
    registry: &mut OutputRegistry,
    single: &Arc<dyn SingleStepExecutor>,
    ctx: &ChainRunContext,
) -> Result<(Vec<StepResult>, Vec<GroupStepResult>), SubagentError> {
    let mut results = Vec::with_capacity(graph.len());
    let mut group_results = Vec::new();

    for (step_index, step) in graph.iter().enumerate() {
        let result = match step {
            RunnerStep::SingleStep(spec) => {
                // C10: resolve {outputs.name}/{task}/{previous}/{chain_dir} + the chain-instruction
                // prefix/suffix before dispatch (fails the whole walk on an unknown {outputs.x}).
                let resolved_task = resolve_step_task(&spec.task, registry, ctx, spec)?;
                let result = single
                    .run_single(spec, &resolved_task, ctx)
                    .await
                    .unwrap_or_else(|err| StepResult::failure(err.to_string()));
                if result.success {
                    // C11: register this step's named output — structured OR plain text — so a later
                    // step's {outputs.name} can resolve it, then advance `previous` for {previous}
                    // piping. On failure neither happens (matching pi, whose failed-summary return
                    // precedes both the `outputs[as] = …` and `prev = …` assignments).
                    register_single_output(registry, spec.output.as_deref(), &result);
                    registry.set_previous(result.final_output.clone().unwrap_or_default());
                }
                result
            }
            RunnerStep::ParallelGroup(spec) => {
                let mut resolved_steps: Vec<SingleStepSpec> =
                    Vec::with_capacity(spec.steps.len());
                for s in &spec.steps {
                    let task = resolve_step_task(&s.task, registry, ctx, s)?;
                    resolved_steps.push(SingleStepSpec {
                        task,
                        ..s.clone()
                    });
                }

                if spec.worktree {
                    assign_worktree_cwds(&mut resolved_steps, ctx).await?;
                }

                // Capture per-task named-output keys and agents before the step list is moved into
                // dispatch_group, so a successful group can register each child's output (C11) and
                // build the aggregated {previous} text.
                let output_names: Vec<Option<String>> =
                    resolved_steps.iter().map(|s| s.output.clone()).collect();
                let agents: Vec<String> =
                    resolved_steps.iter().map(|s| s.agent.clone()).collect();

                let group_result =
                    dispatch_group(resolved_steps, spec.concurrency, spec.fail_fast, single, ctx)
                        .await;
                let collapsed = group_result.aggregate.clone();
                if collapsed.success {
                    for (name, child) in output_names.iter().zip(group_result.children.iter()) {
                        if let (Some(name), Some(child_result)) = (name.as_deref(), child.as_ref()) {
                            register_single_output(registry, Some(name), child_result);
                        }
                    }
                    registry.set_previous(aggregate_group_previous(
                        &group_result.children,
                        &agents,
                    ));
                }
                group_results.push(group_result);
                collapsed
            }
            RunnerStep::DynamicGroup(spec) => {
                let step_display = step_index + 1;
                let item_name = spec.item.as_deref().unwrap_or("item");
                // pi `step.parallel.task ?? "{previous}"`: an omitted (empty) template task
                // defaults to the previous step's output.
                let template_task = if spec.template.task.is_empty() {
                    "{previous}"
                } else {
                    spec.template.task.as_str()
                };
                // pi runs `validateDynamicStepShape` FIRST — it is the opening line of
                // `resolveDynamicFanoutItems` (`dynamic-fanout.ts:217`), BEFORE the expand source is
                // resolved and BEFORE the maxItems / duplicate-key checks. So a malformed or unknown
                // item reference in the RAW template is the first thing rejected, failing the walk
                // ahead of any source-resolution / maxItems / duplicate-key diagnostic — exactly the
                // precedence pi observes. No child is dispatched.
                crate::spawn::dynamic_fanout::assert_no_unresolved_item_references(
                    template_task,
                    item_name,
                    "parallel.task",
                )
                .map_err(SubagentError::StructuredOutputInvalid)?;

                // Resolve the expand source array (an immutable registry borrow that ends with the
                // `.to_vec()`), so the subsequent per-item template resolution + collect
                // registration can re-borrow the registry without overlap. An unresolvable expand
                // pointer aborts the whole walk (hard `Err`), the pre-existing contract this arm's
                // expand-failure tests pin.
                let source: Vec<Value> = registry.resolve_pointer(&spec.expand)?.to_vec();

                // C16: materialize one distinct item per array element — the effective `maxItems`
                // cap (step-level `expand.maxItems`, else the run-wide config fallback), the
                // `expand.key` dedup key, and duplicate-key / colliding-id detection. A
                // materialization failure aborts the walk (hard `Err`,
                // [`SubagentError::StructuredOutputInvalid`]), consistent with the expand-pointer
                // failure above (pi `resolveDynamicFanoutItems`, `dynamic-fanout.ts:216-240`).
                let effective_max = spec.max_items.or(ctx.dynamic_fanout_max_items);
                let items = crate::spawn::dynamic_fanout::resolve_dynamic_fanout_items(
                    &source,
                    spec.key.as_deref(),
                    effective_max,
                    step_display,
                )
                .map_err(SubagentError::StructuredOutputInvalid)?;

                if items.is_empty() {
                    // pi `onEmpty`: `"fail"` errors; `"skip"` (default) registers an empty collect
                    // array and continues, advancing `{previous}` to pi's sentinel text
                    // (`chain-execution.ts:801-841`).
                    if spec.on_empty == OnEmpty::Fail {
                        return Err(SubagentError::StructuredOutputInvalid(format!(
                            "Dynamic chain step {step_display} source array is empty."
                        )));
                    }
                    let collected: Vec<crate::spawn::dynamic_fanout::DynamicCollectedResult> =
                        Vec::new();
                    crate::spawn::dynamic_fanout::validate_dynamic_collection(
                        spec.collect_schema.as_ref(),
                        &collected,
                    )
                    .map_err(SubagentError::StructuredOutputInvalid)?;
                    let collected_value =
                        crate::spawn::dynamic_fanout::collected_results_to_value(&collected);
                    registry.register(spec.collect.clone(), collected_value.clone());
                    registry.set_previous("Dynamic fanout produced 0 results.");
                    // Keep the one-group-result-per-group-step invariant callers rely on
                    // (`render_chain_results` / `runner_main` correlate `group_results` by chain
                    // order): push an empty group result even though nothing was dispatched.
                    group_results.push(GroupStepResult {
                        aggregate: StepResult::success(
                            Some("Dynamic fanout produced 0 results.".to_string()),
                            Some(collected_value.clone()),
                        ),
                        children: Vec::new(),
                    });
                    StepResult::success(
                        Some("Dynamic fanout produced 0 results.".to_string()),
                        Some(collected_value),
                    )
                } else {
                    // Build one distinct, per-item-substituted step spec per element (C16):
                    // item-template substitution first (pi `resolveItemTemplate`), then the flat
                    // `{outputs.name}`/`{task}`/`{previous}`/`{chain_dir}` + chain-instruction pass
                    // ([`resolve_step_task`]), exactly as pi materializes then dispatches — so
                    // every fanned-out child gets its OWN task string.
                    let mut expanded: Vec<SingleStepSpec> = Vec::with_capacity(items.len());
                    for entry in &items {
                        let item_task = crate::spawn::dynamic_fanout::resolve_item_template(
                            template_task,
                            item_name,
                            &entry.item,
                        )
                        .map_err(SubagentError::StructuredOutputInvalid)?;
                        let resolved =
                            resolve_step_task(&item_task, registry, ctx, spec.template.as_ref())?;
                        expanded.push(SingleStepSpec {
                            task: resolved,
                            ..(*spec.template).clone()
                        });
                    }
                    let agents: Vec<String> = expanded.iter().map(|s| s.agent.clone()).collect();

                    let group_result =
                        dispatch_group(expanded, spec.concurrency, false, single, ctx).await;

                    // Fold the per-child results into the ordered collect-record array (pi
                    // `collectDynamicResults`). The narrow [`StepResult`] seam supplies
                    // text/structured/error/agent; `exit_code` is mapped from success (`0`) /
                    // failure (`1`); `timedOut`/`outputPath`/`artifactPaths` are not visible at
                    // this walker layer (the fuller `exec::SingleResult` this crate does not carry
                    // here) and are therefore omitted, not fabricated.
                    let child_inputs: Vec<
                        Option<crate::spawn::dynamic_fanout::CollectChildResult>,
                    > = group_result
                        .children
                        .iter()
                        .map(|child| {
                            child.as_ref().map(|sr| {
                                crate::spawn::dynamic_fanout::CollectChildResult {
                                    agent: Some(spec.template.agent.clone()),
                                    exit_code: Some(i64::from(!sr.success)),
                                    error: sr.error.clone(),
                                    timed_out: false,
                                    structured_output: sr.structured_output.clone(),
                                    artifact_paths: None,
                                    saved_output_path: None,
                                    output: None,
                                    final_output: sr.final_output.clone(),
                                }
                            })
                        })
                        .collect();
                    let collected = crate::spawn::dynamic_fanout::collect_dynamic_results(
                        &items,
                        &child_inputs,
                        &spec.template.agent,
                    );
                    let collected_value =
                        crate::spawn::dynamic_fanout::collected_results_to_value(&collected);

                    // The dynamic step's own aggregate output IS the collect-record array (pi
                    // `outputs[collect.as] = { structured: collected }`), NOT the raw
                    // child-structured-output array `collapse_fan_out` produced.
                    let mut collapsed = group_result.aggregate.clone();
                    collapsed.structured_output = Some(collected_value.clone());

                    if collapsed.success {
                        // pi validates `collect.outputSchema` only on the all-children-succeeded
                        // path (its failure early-return precedes `validateDynamicCollection`). A
                        // schema failure aborts the walk (hard `Err`), matching this arm's
                        // `StructuredOutputInvalid` contract.
                        crate::spawn::dynamic_fanout::validate_dynamic_collection(
                            spec.collect_schema.as_ref(),
                            &collected,
                        )
                        .map_err(SubagentError::StructuredOutputInvalid)?;
                        registry.register(spec.collect.clone(), collected_value);
                        registry.set_previous(aggregate_group_previous(
                            &group_result.children,
                            &agents,
                        ));
                    }
                    group_results.push(group_result);
                    collapsed
                }
            }
            RunnerStep::ImportAsyncRoot(spec) => {
                // R-SA-097 root attachment is resolved by POLLING another run's files, not by
                // spawning a child through the `SingleStepExecutor` seam this walker drives — and
                // `chain_graph` (a `spawn`-module type) deliberately holds no dependency on the
                // `background` module where that poll (`control::wait_for_imported_async_root`)
                // lives. The background runner therefore intercepts an `ImportAsyncRoot` step BEFORE
                // handing the remainder of its chain to any walker, so this arm is unreachable on
                // the real background path. It exists only for exhaustiveness and for the (currently
                // never-constructed) foreground graph that would carry one: rather than silently
                // succeeding, it surfaces a clear failure naming the target run so the divergence is
                // never mistaken for a completed attachment.
                StepResult::failure(format!(
                    "ImportAsyncRoot(run_id={}) must be dispatched by the background runner's \
                     root-attachment poll, not walk_chain",
                    spec.run_id
                ))
            }
        };
        // C9: chain stop-on-failure — a step (or group) that did not succeed halts the walk. The
        // failed [`StepResult`] (carrying its own error text, the "failed summary" the caller renders
        // via pi's `buildChainSummary(…, "failed", { index, error })`) is the last entry in
        // `results`, and no later list element is dispatched — a faithful port of pi's
        // `if (r.exitCode !== 0) return <failed summary>` early return (`chain-execution.ts:1188-1198`).
        // The number of entries in `results` (< `graph.len()` on failure) is itself the signal of
        // exactly where the chain stopped.
        let succeeded = result.success;
        results.push(result);
        if !succeeded {
            break;
        }
    }

    Ok((results, group_results))
}

/// Assign each of `steps`' `cwd` to a dedicated worktree path (R-SA-061), via
/// [`crate::spawn::worktree::setup_worktree_group`] — the exact call every one of that module's
/// own doc comments anticipates this function making. Mutates `steps` in place so the caller's
/// subsequent [`dispatch_group`] call sees the worktree-adjusted `cwd` on every step, exactly as
/// if the chain author had set it directly (except that R-SA-062's own pre-check, run inside
/// `setup_worktree_group` itself before any worktree is created, guarantees no step had already
/// set one).
async fn assign_worktree_cwds(
    steps: &mut [SingleStepSpec],
    ctx: &ChainRunContext,
) -> Result<(), SubagentError> {
    let base_dir = ctx.worktree_base_dir.as_deref().ok_or_else(|| {
        SubagentError::WorktreeSetup(
            "worktree: true group requires ChainRunContext::worktree_base_dir to be configured"
                .to_string(),
        )
    })?;

    let overrides: Vec<Option<&std::path::Path>> =
        steps.iter().map(|s| s.cwd.as_deref()).collect();
    let group_id = uuid::Uuid::now_v7().as_simple().to_string();
    let config = crate::spawn::worktree::WorktreeGroupConfig {
        group_id: &group_id,
        worktree_base_dir: base_dir,
        setup_hook: None,
        setup_hook_timeout_ms: None,
    };

    let plan =
        crate::spawn::worktree::setup_worktree_group(&ctx.cwd, &overrides, &config).await?;

    for (step, assignment) in steps.iter_mut().zip(plan.assignments.iter()) {
        step.cwd = Some(assignment.path.clone());
    }
    Ok(())
}

/// Delegate one group's already-resolved step list to [`crate::spawn::parallel::run_bounded`]
/// (R-SA-049/050/051/066), then collapse its position-ordered [`FanOutResult`] into this file's
/// own [`GroupStepResult`] shape. This is the sole call site in this module that reaches into
/// `spawn::parallel` — both [`RunnerStep::ParallelGroup`] and [`RunnerStep::DynamicGroup`] share
/// it, matching R-SA-052's "walk the list, await each element (or bounded-fan-out group) to
/// completion before moving to the next" algorithm identically for both group shapes.
///
/// `single` is received as an `&Arc<dyn SingleStepExecutor>` and cloned (a cheap refcount bump)
/// into the `'static` worker closure [`run_bounded`] requires, and `ctx` (itself entirely
/// `Clone`, see its own doc comment) is likewise cloned per task — this crate is
/// `#![forbid(unsafe_code)]`, so an owned, `Clone`-based hand-off across the `tokio::spawn`
/// boundary is used instead of any raw-pointer/lifetime-erasure trick.
async fn dispatch_group(
    steps: Vec<SingleStepSpec>,
    concurrency: u32,
    fail_fast: bool,
    single: &Arc<dyn SingleStepExecutor>,
    ctx: &ChainRunContext,
) -> GroupStepResult {
    let cancel = ctx.cancel.clone();
    let global_limit = ctx.global_limit.clone();
    let single = Arc::clone(single);
    let ctx = ctx.clone();

    let fan_out: FanOutResult<StepResult, SubagentError> = run_bounded(
        steps,
        concurrency as usize,
        &global_limit,
        fail_fast,
        cancel,
        move |_index, step: SingleStepSpec| {
            let single = Arc::clone(&single);
            let ctx = ctx.clone();
            async move {
                let resolved_task = step.task.clone();
                single.run_single(&step, &resolved_task, &ctx).await
            }
        },
    )
    .await;

    collapse_fan_out(fan_out)
}

/// Collapse a [`crate::spawn::parallel::run_bounded`] [`FanOutResult`] into this file's
/// [`GroupStepResult`]: `aggregate.success` is `true` iff every dispatched slot succeeded (a
/// skipped slot, R-SA-066/cancellation, counts as NOT succeeded for the aggregate, since the
/// group as a whole did not complete all of its declared work), `aggregate.structured_output` is
/// the JSON array of each child's own `structured_output` (`Value::Null` for a skipped/failed
/// child, so array length/position always matches `children`'s length — R-SA-051's ordering
/// guarantee, restated at the aggregate level), and the full per-child list (including `None` for
/// skipped slots) is retained verbatim in `children`.
fn collapse_fan_out(fan_out: FanOutResult<StepResult, SubagentError>) -> GroupStepResult {
    let children: Vec<Option<StepResult>> = fan_out
        .slots
        .into_iter()
        .map(|slot| match slot {
            Some(Ok(outcome)) => Some(outcome.result),
            Some(Err(err)) => Some(StepResult::failure(err.to_string())),
            None => None,
        })
        .collect();

    let all_populated_and_successful = children
        .iter()
        .all(|c| matches!(c, Some(r) if r.success));
    let success = all_populated_and_successful && !fan_out.any_failed;

    let structured_output = Value::Array(
        children
            .iter()
            .map(|c| {
                c.as_ref()
                    .and_then(|r| r.structured_output.clone())
                    .unwrap_or(Value::Null)
            })
            .collect(),
    );

    let error = if success {
        None
    } else {
        let failed_or_skipped = children.iter().filter(|c| !matches!(c, Some(r) if r.success)).count();
        Some(format!(
            "{failed_or_skipped} of {} group step(s) failed or were skipped",
            children.len()
        ))
    };

    // SUBA-N05: a group's aggregate carries the concatenation of its children's control events, in
    // child order — the aggregate is the only `StepResult` the chain walk records for a group step
    // (`record_step_outcome` folds per-child detail into `status.parallel_groups` separately), so
    // dropping them here would silently lose every event a fanned-out child raised. Upstream keeps
    // per-child events too: its async runner emits one control record per child, keyed by
    // `event.index` (`subagent-runner.ts:2271`), and each cyrup event carries the same `index`.
    let aggregate_control_events: Vec<crate::exec::control::ControlEvent> = children
        .iter()
        .flatten()
        .flat_map(|child| child.control_events.iter().cloned())
        .collect();

    GroupStepResult {
        aggregate: StepResult {
            success,
            structured_output: Some(structured_output),
            final_output: None,
            error,
            interrupted: false,
            control_events: aggregate_control_events,
        },
        children,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn single_step(agent: &str, task: &str) -> SingleStepSpec {
        SingleStepSpec {
            agent: agent.to_string(),
            task: task.to_string(),
            cwd: None,
            model: None,
            tools: None,
            extensions: None,
            session_file: None,
            max_depth_override: None,
            structured_output_schema: None,
            output: None,
            output_path: None,
            output_mode: None,
            reads: None,
            acceptance: None,
            context: None,
            agent_scope: None,
        }
    }

    fn run_ctx(cancel: CancelToken) -> ChainRunContext {
        ChainRunContext {
            cwd: std::env::temp_dir(),
            deadline_at: None,
            timeout_ms: None,
            cancel,
            global_limit: GlobalConcurrencyLimit::default_limit(),
            worktree_base_dir: None,
            original_task: String::new(),
            chain_dir: None,
            dynamic_fanout_max_items: None,
        }
    }

    /// A [`run_ctx`] variant that sets the run-wide `{task}` (original task) and `{chain_dir}` values
    /// so the template-substitution tests can observe them in the resolved child task.
    fn run_ctx_with(
        cancel: CancelToken,
        original_task: &str,
        chain_dir: Option<PathBuf>,
    ) -> ChainRunContext {
        ChainRunContext {
            original_task: original_task.to_string(),
            chain_dir,
            ..run_ctx(cancel)
        }
    }

    /// A [`DynamicGroupSpec`] with a generous `max_items` and pi-default per-item fields, for the
    /// C16 dynamic-fanout walker tests.
    fn dynamic_group(
        expand: &str,
        template: SingleStepSpec,
        collect: &str,
        concurrency: u32,
    ) -> DynamicGroupSpec {
        DynamicGroupSpec {
            expand: expand.to_string(),
            template: Box::new(template),
            collect: collect.to_string(),
            concurrency,
            item: None,
            key: None,
            max_items: Some(8),
            on_empty: OnEmpty::Skip,
            collect_schema: None,
        }
    }

    // ---- Fake SingleStepExecutor recording call order/arguments, standing in for a later
    // phase's exec::run_sync (this file's own delegation-seam contract, never a mock of
    // subprocess behavior itself — this crate's convention of not mocking subprocess/git/
    // filesystem behavior applies to `spawn/mod.rs`/`spawn/parallel.rs`/`spawn/worktree.rs`'s
    // own tests, which exercise real child processes/real git; this file's concern is purely the
    // walker's control flow over the `SingleStepExecutor` seam plus real delegation to the ALSO
    // real `spawn::parallel::run_bounded`, which is exercised for real in every test below — only
    // the innermost "spawn an actual OS subprocess" step is faked, exactly mirroring
    // `spawn::parallel`'s own tests' use of a plain async closure worker in place of a real
    // child). ----

    /// Records, in call order, the resolved task text actually observed by each dispatched
    /// single-step — proving both linear order and that template resolution (`{outputs.name}` /
    /// `{previous}` / `{task}` / `{chain_dir}` / instruction prefix+suffix) happened before
    /// dispatch. Optional per-task maps let a test pin a step's structured output, its plain-text
    /// final output, or force it to fail (for the stop-on-failure walk). Keys are matched against
    /// the FULLY RESOLVED task text a step is dispatched with — a substring match, so a test need
    /// not reconstruct the instruction prefix/suffix build_chain_instructions appends.
    #[derive(Default)]
    struct RecordingExecutor {
        calls: StdMutex<Vec<String>>,
        structured_output_for: StdMutex<std::collections::HashMap<String, Value>>,
        final_output_for: StdMutex<std::collections::HashMap<String, String>>,
        fail_if_contains: StdMutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl SingleStepExecutor for RecordingExecutor {
        async fn run_single(
            &self,
            _step: &SingleStepSpec,
            resolved_task: &str,
            _ctx: &ChainRunContext,
        ) -> Result<StepResult, SubagentError> {
            self.calls
                .lock()
                .expect("lock")
                .push(resolved_task.to_string());
            if let Some(needle) = self
                .fail_if_contains
                .lock()
                .expect("lock")
                .iter()
                .find(|needle| resolved_task.contains(needle.as_str()))
            {
                return Ok(StepResult::failure(format!(
                    "forced failure for task containing {needle:?}"
                )));
            }
            let structured_output = self
                .structured_output_for
                .lock()
                .expect("lock")
                .iter()
                .find(|(needle, _)| resolved_task.contains(needle.as_str()))
                .map(|(_, value)| value.clone());
            let final_output = self
                .final_output_for
                .lock()
                .expect("lock")
                .iter()
                .find(|(needle, _)| resolved_task.contains(needle.as_str()))
                .map_or_else(|| "ok".to_string(), |(_, text)| text.clone());
            Ok(StepResult::success(Some(final_output), structured_output))
        }
    }

    // ---- R-SA-052: linear walk order is respected ----

    #[tokio::test]
    async fn walk_chain_visits_every_step_in_list_order() {
        let graph: ChainGraph = vec![
            RunnerStep::SingleStep(single_step("researcher", "first")),
            RunnerStep::SingleStep(single_step("writer", "second")),
            RunnerStep::SingleStep(single_step("reviewer", "third")),
        ];
        let executor = Arc::new(RecordingExecutor::default());
        let executor_dyn: Arc<dyn SingleStepExecutor> = executor.clone();
        let cancel = CancelToken::new();
        let ctx = run_ctx(cancel);
        let mut registry = OutputRegistry::new();

        let (results, _groups) = walk_chain(&graph, &mut registry, &executor_dyn, &ctx)
            .await
            .expect("walk succeeds");

        assert_eq!(results.len(), 3, "one StepResult per RunnerStep, in order");
        assert!(results.iter().all(|r| r.success));
        // Each dispatched task begins with its own template text (a trailing "Previous step output"
        // suffix may follow from the C10 default-{previous} piping), and they appear in list order.
        let calls = executor.calls.lock().expect("lock").clone();
        assert_eq!(calls.len(), 3);
        assert!(calls[0].starts_with("first"));
        assert!(calls[1].starts_with("second"));
        assert!(calls[2].starts_with("third"));
    }

    #[tokio::test]
    async fn walk_chain_never_dispatches_the_trailing_single_step_before_the_group_completes() {
        let graph: ChainGraph = vec![
            RunnerStep::SingleStep(single_step("a", "step-1")),
            RunnerStep::ParallelGroup(ParallelGroupSpec {
                steps: vec![single_step("b", "p-1"), single_step("b", "p-2")],
                concurrency: 2,
                fail_fast: false,
                worktree: false,
            }),
            RunnerStep::SingleStep(single_step("c", "step-3")),
        ];
        let executor = Arc::new(RecordingExecutor::default());
        let executor_dyn: Arc<dyn SingleStepExecutor> = executor.clone();
        let cancel = CancelToken::new();
        let ctx = run_ctx(cancel);
        let mut registry = OutputRegistry::new();

        let (results, groups) = walk_chain(&graph, &mut registry, &executor_dyn, &ctx)
            .await
            .expect("walk succeeds");

        assert_eq!(results.len(), 3);
        assert_eq!(groups.len(), 1, "exactly one group step in this chain");
        // `step-3` must appear strictly after both `p-1`/`p-2` in the call log, proving the
        // group was awaited to completion before the walker proceeded.
        let calls = executor.calls.lock().expect("lock").clone();
        // `starts_with` rather than exact equality: the C10 default-{previous} piping may append a
        // "Previous step output" suffix to `p-*`/`step-3`'s resolved task.
        let step3_pos = calls
            .iter()
            .position(|c| c.starts_with("step-3"))
            .expect("present");
        let p1_pos = calls.iter().position(|c| c.starts_with("p-1")).expect("present");
        let p2_pos = calls.iter().position(|c| c.starts_with("p-2")).expect("present");
        assert!(step3_pos > p1_pos && step3_pos > p2_pos);
        assert!(calls[0].starts_with("step-1"));
    }

    // ---- ParallelGroup delegates to spawn::parallel::run_bounded (the REAL primitive) ----

    #[tokio::test]
    async fn parallel_group_step_delegates_to_run_bounded_with_its_full_step_list() {
        let graph: ChainGraph = vec![RunnerStep::ParallelGroup(ParallelGroupSpec {
            steps: vec![
                single_step("worker", "task-a"),
                single_step("worker", "task-b"),
                single_step("worker", "task-c"),
            ],
            concurrency: 3,
            fail_fast: false,
            worktree: false,
        })];
        let executor = Arc::new(RecordingExecutor::default());
        let executor_dyn: Arc<dyn SingleStepExecutor> = executor.clone();
        let cancel = CancelToken::new();
        let ctx = run_ctx(cancel);
        let mut registry = OutputRegistry::new();

        let (results, groups) = walk_chain(&graph, &mut registry, &executor_dyn, &ctx)
            .await
            .expect("walk succeeds");

        assert_eq!(results.len(), 1, "the whole ParallelGroup is one RunnerStep");
        assert!(results[0].success);
        assert_eq!(groups.len(), 1);
        assert_eq!(
            groups[0].children.len(),
            3,
            "run_bounded must have produced one slot per group step"
        );
        assert!(groups[0].children.iter().all(|c| matches!(c, Some(r) if r.success)));

        let mut calls = executor.calls.lock().expect("lock").clone();
        calls.sort();
        assert_eq!(
            calls,
            vec![
                "task-a".to_string(),
                "task-b".to_string(),
                "task-c".to_string()
            ],
            "every one of the group's 3 steps must have been dispatched via run_bounded"
        );
    }

    #[tokio::test]
    async fn parallel_group_result_ordering_matches_input_order_regardless_of_completion_order() {
        // A worker that completes tasks out of order (via a per-task variable, tiny, deterministic
        // delay keyed on task content) must still produce a position-ordered `children` list
        // (R-SA-051), proven through the REAL `run_bounded` primitive this file delegates to.
        struct VariableDelayExecutor;
        #[async_trait::async_trait]
        impl SingleStepExecutor for VariableDelayExecutor {
            async fn run_single(
                &self,
                step: &SingleStepSpec,
                resolved_task: &str,
                _ctx: &ChainRunContext,
            ) -> Result<StepResult, SubagentError> {
                // Reverse-order delay: the FIRST task sleeps longest, so completion order is the
                // reverse of input order — yet result ordering must still match input order.
                let delay_ms = match step.task.as_str() {
                    "first" => 30,
                    "second" => 15,
                    _ => 0,
                };
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                Ok(StepResult::success(
                    Some(resolved_task.to_string()),
                    None,
                ))
            }
        }

        let graph: ChainGraph = vec![RunnerStep::ParallelGroup(ParallelGroupSpec {
            steps: vec![
                single_step("w", "first"),
                single_step("w", "second"),
                single_step("w", "third"),
            ],
            concurrency: 3,
            fail_fast: false,
            worktree: false,
        })];
        let executor = Arc::new(VariableDelayExecutor);
        let executor_dyn: Arc<dyn SingleStepExecutor> = executor.clone();
        let cancel = CancelToken::new();
        let ctx = run_ctx(cancel);
        let mut registry = OutputRegistry::new();

        let (_results, groups) = walk_chain(&graph, &mut registry, &executor_dyn, &ctx)
            .await
            .expect("walk succeeds");

        let outputs: Vec<Option<String>> = groups[0]
            .children
            .iter()
            .map(|c| c.as_ref().and_then(|r| r.final_output.clone()))
            .collect();
        assert_eq!(
            outputs,
            vec![
                Some("first".to_string()),
                Some("second".to_string()),
                Some("third".to_string())
            ],
            "children must be in original input order even though \"first\" finishes LAST"
        );
    }

    // ---- R-SA-053 / C11: named-output resolution against strictly earlier steps only ----

    /// Required T4 test: a **plain-text** output (no structured JSON) registered by an earlier step
    /// feeds the next step's `{outputs.name}` reference (mirrors pi's `passes named sequential
    /// outputs through {outputs.name}` — `chain-execution.test.ts:537-555` — which produces a text
    /// output "Context marker: CTX_123" and asserts the writer's task contains it).
    #[tokio::test]
    async fn a_text_output_feeds_the_next_steps_outputs_reference() {
        let mut first = single_step("researcher", "find the answer");
        first.output = Some("finding".to_string());
        let second = single_step("writer", "write about {outputs.finding}");
        let graph: ChainGraph = vec![
            RunnerStep::SingleStep(first),
            RunnerStep::SingleStep(second),
        ];
        let executor = Arc::new(RecordingExecutor::default());
        let executor_dyn: Arc<dyn SingleStepExecutor> = executor.clone();
        // Step one produces PLAIN TEXT (no structured output) — the case C11 adds registry support
        // for. The prior implementation only registered structured outputs, so this reference would
        // have gone unresolved.
        executor
            .final_output_for
            .lock()
            .expect("lock")
            .insert("find the answer".to_string(), "42".to_string());
        let cancel = CancelToken::new();
        let ctx = run_ctx(cancel);
        let mut registry = OutputRegistry::new();

        walk_chain(&graph, &mut registry, &executor_dyn, &ctx)
            .await
            .expect("walk succeeds");

        let calls = executor.calls.lock().expect("lock").clone();
        assert!(calls[0].contains("find the answer"));
        assert!(
            calls[1].contains("write about 42"),
            "the second step's {{outputs.finding}} reference must resolve to the first step's \
             registered PLAIN-TEXT output (C11), got: {:?}",
            calls[1]
        );
        // The registry entry is a text entry (no structured value), so a dynamic expand over it
        // would fail — but a {outputs.name} reference resolves fine.
        let entry = registry.get("finding").expect("registered");
        assert_eq!(entry.text, "42");
        assert_eq!(entry.structured, None);
    }

    /// Required T4 test (the C11 hard-failure half): a `{outputs.x}` reference to an output no
    /// strictly-earlier step produced is a hard error carrying pi's exact message — not a silent
    /// pass-through of the literal placeholder (pi `resolveOutputReferences`,
    /// `chain-outputs.ts:91` throws `Unknown chain output reference`).
    #[tokio::test]
    async fn an_unregistered_output_reference_is_a_hard_error() {
        let registry = OutputRegistry::new();
        let err = registry
            .resolve("use {outputs.never_registered} here")
            .expect_err("an unknown output reference must be a hard error");
        match err {
            SubagentError::ChainOutputInvalid(msg) => assert_eq!(
                msg,
                "Unknown chain output reference '{outputs.never_registered}'.",
                "must carry pi's exact ChainOutputValidationError message"
            ),
            other => panic!("expected ChainOutputInvalid, got {other:?}"),
        }
    }

    /// A `walk_chain` that dispatches a step whose task references an unregistered output fails the
    /// whole walk BEFORE spawning that step (the resolution error propagates out of the walker),
    /// exactly as pi refuses to run a chain with an unknown output binding.
    #[tokio::test]
    async fn walk_chain_fails_before_dispatch_on_an_unknown_output_reference() {
        let graph: ChainGraph =
            vec![RunnerStep::SingleStep(single_step("writer", "use {outputs.missing}"))];
        let executor = Arc::new(RecordingExecutor::default());
        let executor_dyn: Arc<dyn SingleStepExecutor> = executor.clone();
        let cancel = CancelToken::new();
        let ctx = run_ctx(cancel);
        let mut registry = OutputRegistry::new();

        let err = walk_chain(&graph, &mut registry, &executor_dyn, &ctx)
            .await
            .expect_err("an unknown {outputs.x} reference must fail the walk");
        assert!(matches!(err, SubagentError::ChainOutputInvalid(_)));
        assert!(
            executor.calls.lock().expect("lock").is_empty(),
            "the step referencing an unknown output must never be dispatched"
        );
    }

    /// An invalid output-reference NAME (not matching `/^[A-Za-z_][A-Za-z0-9_]*$/`) is a hard error
    /// with pi's exact message (pi `resolveOutputReferences`, `chain-outputs.ts:88`).
    #[tokio::test]
    async fn a_malformed_output_reference_name_is_a_hard_error() {
        let registry = OutputRegistry::new();
        let err = registry
            .resolve("use {outputs.bad-name} here")
            .expect_err("a malformed output-reference name must be a hard error");
        match err {
            SubagentError::ChainOutputInvalid(msg) => assert_eq!(
                msg,
                "Invalid chain output reference '{outputs.bad-name}'. Use {outputs.name} \
                 with /^[A-Za-z_][A-Za-z0-9_]*$/ names."
            ),
            other => panic!("expected ChainOutputInvalid, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn output_registered_by_step_one_is_not_visible_before_step_one_completes() {
        // Structural proof of "strictly earlier only": resolve() against a registry that has not
        // yet had step one's output registered must not find it, even though the SAME name will
        // be registered moments later — there is no way to reach it early. Under C11 an unresolved
        // reference is now a hard error (not a silent pass-through), so this asserts the error.
        let registry = OutputRegistry::new();
        assert_eq!(registry.get("finding"), None);
        assert!(matches!(
            registry.resolve("{outputs.finding}"),
            Err(SubagentError::ChainOutputInvalid(_))
        ));
    }

    // ---- C10: {previous} / {task} / {chain_dir} template substitution ----

    /// Required T4 test: a step reads `{previous}` — step 2 (whose task IS `{previous}`) receives
    /// step 1's output (mirrors pi's `passes {previous} between steps` —
    /// `chain-execution.test.ts:518-535`).
    #[tokio::test]
    async fn a_step_reads_previous_from_the_prior_step() {
        let step1 = single_step("step1", "produce output");
        let step2 = single_step("step2", "{previous}");
        let graph: ChainGraph = vec![
            RunnerStep::SingleStep(step1),
            RunnerStep::SingleStep(step2),
        ];
        let executor = Arc::new(RecordingExecutor::default());
        let executor_dyn: Arc<dyn SingleStepExecutor> = executor.clone();
        executor.final_output_for.lock().expect("lock").insert(
            "produce output".to_string(),
            "Step 1 unique output: MARKER_ABC_123".to_string(),
        );
        let cancel = CancelToken::new();
        let ctx = run_ctx(cancel);
        let mut registry = OutputRegistry::new();

        walk_chain(&graph, &mut registry, &executor_dyn, &ctx)
            .await
            .expect("walk succeeds");

        let calls = executor.calls.lock().expect("lock").clone();
        assert!(
            calls[1].contains("MARKER_ABC_123"),
            "step 2 must receive step 1's output via {{previous}}, got: {:?}",
            calls[1]
        );
        // An EXPLICIT {previous} is substituted inline and NOT also appended as a "Previous step
        // output" suffix (pi's `templateHasPrevious ? undefined : prev`).
        assert!(
            !calls[1].contains("Previous step output:"),
            "an explicit {{previous}} must not also trigger the previous-summary suffix"
        );
    }

    /// The default `{previous}` piping: a step with no explicit `{previous}` placeholder still
    /// receives the prior step's output, appended by `build_chain_instructions` as a
    /// `Previous step output:` suffix (pi passes `prev` as `previousSummary` when the template lacks
    /// `{previous}`).
    #[tokio::test]
    async fn the_previous_summary_is_appended_when_the_template_has_no_previous_placeholder() {
        let step1 = single_step("step1", "produce");
        let step2 = single_step("step2", "do more work");
        let graph: ChainGraph = vec![
            RunnerStep::SingleStep(step1),
            RunnerStep::SingleStep(step2),
        ];
        let executor = Arc::new(RecordingExecutor::default());
        let executor_dyn: Arc<dyn SingleStepExecutor> = executor.clone();
        executor
            .final_output_for
            .lock()
            .expect("lock")
            .insert("produce".to_string(), "OUTPUT_MARKER".to_string());
        let cancel = CancelToken::new();
        let ctx = run_ctx(cancel);
        let mut registry = OutputRegistry::new();

        walk_chain(&graph, &mut registry, &executor_dyn, &ctx)
            .await
            .expect("walk succeeds");

        let calls = executor.calls.lock().expect("lock").clone();
        assert!(calls[1].starts_with("do more work"));
        assert!(
            calls[1].contains("Previous step output:\nOUTPUT_MARKER"),
            "the prior output must be appended as a previous-summary suffix, got: {:?}",
            calls[1]
        );
    }

    /// `{task}` resolves to the chain's original top-level task (pi's `substitutes {task} in
    /// templates` — `chain-execution.test.ts:879-897`).
    #[tokio::test]
    async fn substitutes_task_placeholder_with_the_original_task() {
        let graph: ChainGraph =
            vec![RunnerStep::SingleStep(single_step("worker", "Review {task} carefully"))];
        let executor = Arc::new(RecordingExecutor::default());
        let executor_dyn: Arc<dyn SingleStepExecutor> = executor.clone();
        let cancel = CancelToken::new();
        let ctx = run_ctx_with(cancel, "the authentication module", None);
        let mut registry = OutputRegistry::new();

        walk_chain(&graph, &mut registry, &executor_dyn, &ctx)
            .await
            .expect("walk succeeds");

        let calls = executor.calls.lock().expect("lock").clone();
        assert!(
            calls[0].contains("Review the authentication module carefully"),
            "{{task}} must substitute the original task, got: {:?}",
            calls[0]
        );
    }

    /// `{chain_dir}` resolves to the chain working directory (pi's `creates and uses chain_dir` —
    /// `chain-execution.test.ts:899-914`).
    #[tokio::test]
    async fn substitutes_chain_dir_placeholder() {
        let chain_dir = std::env::temp_dir().join("cyrup-chain-dir-marker");
        let graph: ChainGraph =
            vec![RunnerStep::SingleStep(single_step("worker", "Write to {chain_dir}"))];
        let executor = Arc::new(RecordingExecutor::default());
        let executor_dyn: Arc<dyn SingleStepExecutor> = executor.clone();
        let cancel = CancelToken::new();
        let ctx = run_ctx_with(cancel, "", Some(chain_dir.clone()));
        let mut registry = OutputRegistry::new();

        walk_chain(&graph, &mut registry, &executor_dyn, &ctx)
            .await
            .expect("walk succeeds");

        let calls = executor.calls.lock().expect("lock").clone();
        assert!(
            calls[0].contains(&chain_dir.to_string_lossy().into_owned()),
            "{{chain_dir}} must substitute the chain directory, got: {:?}",
            calls[0]
        );
    }

    #[test]
    fn build_chain_instructions_emits_reads_output_prefix_and_previous_suffix() {
        let (prefix, suffix) = build_chain_instructions(
            Some(&[PathBuf::from("notes.md")]),
            Some("out.md"),
            Path::new("/chain"),
            Some("prev text"),
        );
        assert_eq!(prefix, "[Read from: /chain/notes.md]\n[Write to: /chain/out.md]\n\n");
        assert_eq!(suffix, "\n\n---\nPrevious step output:\nprev text");

        // No reads/output/previous → empty prefix and suffix, and a blank previous summary is
        // dropped (pi's `previousSummary.trim()` guard).
        let (prefix, suffix) = build_chain_instructions(None, None, Path::new("/chain"), Some("   "));
        assert_eq!(prefix, "");
        assert_eq!(suffix, "");
    }

    // ---- C9: chain stop-on-failure ----

    /// Required T4 test: a failed step halts the chain — no later step is dispatched and exactly one
    /// (failed) result is returned (mirrors pi's `stops chain on step failure` —
    /// `chain-execution.test.ts:916-930`).
    #[tokio::test]
    async fn a_failed_step_halts_the_chain() {
        let graph: ChainGraph = vec![
            RunnerStep::SingleStep(single_step("step1", "do first thing")),
            RunnerStep::SingleStep(single_step("step2", "do second thing")),
        ];
        let executor = Arc::new(RecordingExecutor::default());
        let executor_dyn: Arc<dyn SingleStepExecutor> = executor.clone();
        executor
            .fail_if_contains
            .lock()
            .expect("lock")
            .push("do first thing".to_string());
        let cancel = CancelToken::new();
        let ctx = run_ctx(cancel);
        let mut registry = OutputRegistry::new();

        let (results, _groups) = walk_chain(&graph, &mut registry, &executor_dyn, &ctx)
            .await
            .expect("walk itself returns Ok; the step failure is carried in the StepResult");

        assert_eq!(results.len(), 1, "only step1 should have run before the chain halted");
        assert!(!results[0].success, "step1's failure must be recorded");
        assert!(results[0].error.is_some(), "the failed step carries its error summary");
        let calls = executor.calls.lock().expect("lock").clone();
        assert_eq!(calls.len(), 1);
        assert!(
            !calls.iter().any(|c| c.contains("do second thing")),
            "step2 must never be dispatched after step1 fails"
        );
    }

    /// A step that succeeds after an earlier step failed is never reached — and, symmetrically, the
    /// failed step does not advance `{previous}` or register its `as` output (pi returns the failed
    /// summary before its `outputs[as] = …` / `prev = …` assignments).
    #[tokio::test]
    async fn a_failed_step_does_not_register_its_output_or_advance_previous() {
        let mut failing = single_step("step1", "do first thing");
        failing.output = Some("shouldNotRegister".to_string());
        let graph: ChainGraph = vec![RunnerStep::SingleStep(failing)];
        let executor = Arc::new(RecordingExecutor::default());
        let executor_dyn: Arc<dyn SingleStepExecutor> = executor.clone();
        executor
            .fail_if_contains
            .lock()
            .expect("lock")
            .push("do first thing".to_string());
        let cancel = CancelToken::new();
        let ctx = run_ctx(cancel);
        let mut registry = OutputRegistry::new();

        walk_chain(&graph, &mut registry, &executor_dyn, &ctx)
            .await
            .expect("walk returns Ok");

        assert_eq!(registry.get("shouldNotRegister"), None);
        assert_eq!(registry.previous(), "");
    }

    // ---- C11: parallel per-task named outputs feed later {outputs.name} references ----

    /// Required-adjacent T4 test: a parallel step's per-task `as` outputs are registered so a later
    /// sequential step's `{outputs.name}` references resolve (mirrors pi's `passes completed parallel
    /// task outputs to later {outputs.name} references` — `chain-execution.test.ts:1197-1222`). The
    /// prior implementation registered NO parallel-child outputs.
    #[tokio::test]
    async fn parallel_per_task_named_outputs_feed_a_later_outputs_reference() {
        let mut alpha = single_step("alpha", "Alpha");
        alpha.output = Some("alphaOutput".to_string());
        let mut beta = single_step("beta", "Beta");
        beta.output = Some("betaOutput".to_string());
        let graph: ChainGraph = vec![
            RunnerStep::ParallelGroup(ParallelGroupSpec {
                steps: vec![alpha, beta],
                concurrency: 2,
                fail_fast: false,
                worktree: false,
            }),
            RunnerStep::SingleStep(single_step(
                "writer",
                "Use {outputs.alphaOutput} and {outputs.betaOutput}",
            )),
        ];
        let executor = Arc::new(RecordingExecutor::default());
        let executor_dyn: Arc<dyn SingleStepExecutor> = executor.clone();
        {
            let mut outs = executor.final_output_for.lock().expect("lock");
            outs.insert("Alpha".to_string(), "Alpha named output".to_string());
            outs.insert("Beta".to_string(), "Beta named output".to_string());
        }
        let cancel = CancelToken::new();
        let ctx = run_ctx(cancel);
        let mut registry = OutputRegistry::new();

        walk_chain(&graph, &mut registry, &executor_dyn, &ctx)
            .await
            .expect("walk succeeds");

        let calls = executor.calls.lock().expect("lock").clone();
        let writer_task = calls.last().expect("writer dispatched");
        assert!(
            writer_task.contains("Alpha named output"),
            "the parallel alpha task's named output must feed the later {{outputs.alphaOutput}}: {writer_task:?}"
        );
        assert!(
            writer_task.contains("Beta named output"),
            "the parallel beta task's named output must feed the later {{outputs.betaOutput}}: {writer_task:?}"
        );
    }

    // ---- DynamicGroup: expand resolves only against validated structured output ----

    #[tokio::test]
    async fn dynamic_group_expands_over_a_prior_steps_structured_output_array() {
        let mut source = single_step("planner", "make a plan");
        source.output = Some("plan".to_string());
        let dynamic = dynamic_group(
            "outputs.plan",
            single_step("worker", "handle one item"),
            "results",
            4,
        );
        let graph: ChainGraph = vec![
            RunnerStep::SingleStep(source),
            RunnerStep::DynamicGroup(dynamic),
        ];
        let executor = Arc::new(RecordingExecutor::default());
        let executor_dyn: Arc<dyn SingleStepExecutor> = executor.clone();
        executor.structured_output_for.lock().expect("lock").insert(
            "make a plan".to_string(),
            serde_json::json!([{ "id": 1 }, { "id": 2 }, { "id": 3 }]),
        );
        let cancel = CancelToken::new();
        let ctx = run_ctx(cancel);
        let mut registry = OutputRegistry::new();

        let (results, groups) = walk_chain(&graph, &mut registry, &executor_dyn, &ctx)
            .await
            .expect("walk succeeds");

        assert_eq!(results.len(), 2);
        assert_eq!(
            groups[0].children.len(),
            3,
            "the dynamic group must expand to exactly 3 steps, matching the resolved array length"
        );
        assert!(
            registry.get("results").is_some(),
            "the collect-named aggregate output must be registered after the group completes"
        );
    }

    #[tokio::test]
    async fn dynamic_group_expand_pointer_walks_a_nested_object_field() {
        let mut source = single_step("planner", "make a plan");
        source.output = Some("plan".to_string());
        let dynamic = dynamic_group(
            "outputs.plan/items",
            single_step("worker", "handle one item"),
            "results",
            2,
        );
        let graph: ChainGraph = vec![
            RunnerStep::SingleStep(source),
            RunnerStep::DynamicGroup(dynamic),
        ];
        let executor = Arc::new(RecordingExecutor::default());
        let executor_dyn: Arc<dyn SingleStepExecutor> = executor.clone();
        executor.structured_output_for.lock().expect("lock").insert(
            "make a plan".to_string(),
            serde_json::json!({
                "items": [ { "id": 1 }, { "id": 2 } ],
                "other": "ignored"
            }),
        );
        let cancel = CancelToken::new();
        let ctx = run_ctx(cancel);
        let mut registry = OutputRegistry::new();

        let (results, groups) = walk_chain(&graph, &mut registry, &executor_dyn, &ctx)
            .await
            .expect("walk succeeds");

        assert_eq!(results.len(), 2);
        assert_eq!(groups[0].children.len(), 2);
    }

    #[tokio::test]
    async fn dynamic_group_expand_against_an_unregistered_output_fails_the_whole_walk() {
        let dynamic = dynamic_group(
            "outputs.never_produced",
            single_step("worker", "handle one item"),
            "results",
            2,
        );
        let graph: ChainGraph = vec![RunnerStep::DynamicGroup(dynamic)];
        let executor = Arc::new(RecordingExecutor::default());
        let executor_dyn: Arc<dyn SingleStepExecutor> = executor.clone();
        let cancel = CancelToken::new();
        let ctx = run_ctx(cancel);
        let mut registry = OutputRegistry::new();

        let err = walk_chain(&graph, &mut registry, &executor_dyn, &ctx)
            .await
            .expect_err("expand against a non-existent output must fail the walk");
        assert!(matches!(err, SubagentError::StructuredOutputInvalid(_)));
        assert!(
            executor.calls.lock().expect("lock").is_empty(),
            "run_bounded (and therefore run_single) must never be called when expand \
             resolution itself fails"
        );
    }

    #[tokio::test]
    async fn dynamic_group_expand_against_a_non_array_value_fails() {
        let mut source = single_step("planner", "make a plan");
        source.output = Some("plan".to_string());
        let dynamic = dynamic_group(
            "outputs.plan",
            single_step("worker", "handle one item"),
            "results",
            2,
        );
        let graph: ChainGraph = vec![
            RunnerStep::SingleStep(source),
            RunnerStep::DynamicGroup(dynamic),
        ];
        let executor = Arc::new(RecordingExecutor::default());
        let executor_dyn: Arc<dyn SingleStepExecutor> = executor.clone();
        executor.structured_output_for.lock().expect("lock").insert(
            "make a plan".to_string(),
            serde_json::json!({ "not": "an array" }),
        );
        let cancel = CancelToken::new();
        let ctx = run_ctx(cancel);
        let mut registry = OutputRegistry::new();

        let err = walk_chain(&graph, &mut registry, &executor_dyn, &ctx)
            .await
            .expect_err("a non-array resolved value must fail");
        assert!(matches!(err, SubagentError::StructuredOutputInvalid(_)));
    }

    // ---- C16: per-item dynamic fan-out (dynamic-fanout.ts:137-240) ----

    /// A [`DynamicGroupSpec`] exposing the C16 per-item fields (`item`/`key`/`maxItems`/`onEmpty`).
    fn dynamic_group_full(
        expand: &str,
        template: SingleStepSpec,
        collect: &str,
        item: Option<&str>,
        key: Option<&str>,
        max_items: Option<u32>,
        on_empty: OnEmpty,
    ) -> DynamicGroupSpec {
        DynamicGroupSpec {
            expand: expand.to_string(),
            template: Box::new(template),
            collect: collect.to_string(),
            concurrency: 4,
            item: item.map(str::to_string),
            key: key.map(str::to_string),
            max_items,
            on_empty,
            collect_schema: None,
        }
    }

    #[tokio::test]
    async fn dynamic_group_gives_each_child_its_own_substituted_task() {
        // The C16 gap: previously every fanned child got the identical task string. Now each child
        // gets `{t.path}` substituted from its own array element.
        let dynamic = dynamic_group_full(
            "outputs.targets",
            single_step("reviewer", "Review {t.path}"),
            "reviews",
            Some("t"),
            Some("/path"),
            Some(4),
            OnEmpty::Skip,
        );
        let graph: ChainGraph = vec![RunnerStep::DynamicGroup(dynamic)];
        let executor = Arc::new(RecordingExecutor::default());
        let executor_dyn: Arc<dyn SingleStepExecutor> = executor.clone();
        let ctx = run_ctx(CancelToken::new());
        let mut registry = OutputRegistry::new();
        registry.register(
            "targets",
            serde_json::json!([{ "path": "src/a.ts" }, { "path": "src/b.ts" }]),
        );

        let (results, groups) = walk_chain(&graph, &mut registry, &executor_dyn, &ctx)
            .await
            .expect("walk succeeds");

        assert_eq!(results.len(), 1);
        assert_eq!(groups[0].children.len(), 2, "one child per source item");
        let mut calls = executor.calls.lock().expect("lock").clone();
        calls.sort();
        assert!(
            calls[0].contains("Review src/a.ts"),
            "first child's task must be substituted from its own item: {calls:?}"
        );
        assert!(
            calls[1].contains("Review src/b.ts"),
            "second child's task must be substituted from its own item: {calls:?}"
        );
    }

    #[tokio::test]
    async fn dynamic_group_max_items_caps_the_fan_out() {
        let dynamic = dynamic_group_full(
            "outputs.targets",
            single_step("reviewer", "Review {t.path}"),
            "reviews",
            Some("t"),
            Some("/path"),
            Some(1), // cap below the 2-element array
            OnEmpty::Skip,
        );
        let graph: ChainGraph = vec![RunnerStep::DynamicGroup(dynamic)];
        let executor = Arc::new(RecordingExecutor::default());
        let executor_dyn: Arc<dyn SingleStepExecutor> = executor.clone();
        let ctx = run_ctx(CancelToken::new());
        let mut registry = OutputRegistry::new();
        registry.register("targets", serde_json::json!([{ "path": "a" }, { "path": "b" }]));

        let err = walk_chain(&graph, &mut registry, &executor_dyn, &ctx)
            .await
            .expect_err("an over-limit array must fail the walk");
        assert!(
            matches!(&err, SubagentError::StructuredOutputInvalid(m) if m.contains("exceeding maxItems")),
            "got: {err:?}"
        );
        assert!(
            executor.calls.lock().expect("lock").is_empty(),
            "no child may be dispatched once the maxItems cap is exceeded"
        );
    }

    /// C16 fallback (pi `config.chain.dynamicFanout.maxItems`): a step whose own `expand.maxItems`
    /// is absent must fall back to [`ChainRunContext::dynamic_fanout_max_items`] — the run-wide cap
    /// the orchestrator resolved from config and threaded in via `RunnerConfig`/
    /// `run_chain_foreground`. Regression proof for the `ChainRunContext`/`RunnerConfig` struct-
    /// literal wiring: if either caller ever again constructs the context without populating this
    /// field (e.g. reverting to a hardcoded `None`, as a careless fix of a missing-field compile
    /// error could do), this test fails with "requires an effective maxItems" instead of the
    /// expected "exceeding maxItems 1" — proving the run-wide cap actually reached the walker
    /// rather than silently defaulting to "no cap configured".
    #[tokio::test]
    async fn dynamic_group_falls_back_to_ctx_wide_max_items_when_step_omits_it() {
        let dynamic = dynamic_group_full(
            "outputs.targets",
            single_step("reviewer", "Review {t.path}"),
            "reviews",
            Some("t"),
            Some("/path"),
            None, // step itself sets no maxItems — must fall back to ctx
            OnEmpty::Skip,
        );
        let graph: ChainGraph = vec![RunnerStep::DynamicGroup(dynamic)];
        let executor = Arc::new(RecordingExecutor::default());
        let executor_dyn: Arc<dyn SingleStepExecutor> = executor.clone();
        let ctx = ChainRunContext {
            dynamic_fanout_max_items: Some(1), // run-wide cap below the 2-element array
            ..run_ctx(CancelToken::new())
        };
        let mut registry = OutputRegistry::new();
        registry.register("targets", serde_json::json!([{ "path": "a" }, { "path": "b" }]));

        let err = walk_chain(&graph, &mut registry, &executor_dyn, &ctx)
            .await
            .expect_err("the ctx-wide cap must be applied when the step omits its own maxItems");
        assert!(
            matches!(&err, SubagentError::StructuredOutputInvalid(m) if m.contains("exceeding maxItems")),
            "expected the ctx-wide cap (1) to reject the 2-element array, got: {err:?}"
        );
        assert!(
            executor.calls.lock().expect("lock").is_empty(),
            "no child may be dispatched once the ctx-wide maxItems cap is exceeded"
        );
    }

    #[tokio::test]
    async fn dynamic_group_duplicate_keys_fail_the_walk() {
        let dynamic = dynamic_group_full(
            "outputs.targets",
            single_step("reviewer", "Review {t.path}"),
            "reviews",
            Some("t"),
            Some("/path"),
            Some(4),
            OnEmpty::Skip,
        );
        let graph: ChainGraph = vec![RunnerStep::DynamicGroup(dynamic)];
        let executor = Arc::new(RecordingExecutor::default());
        let executor_dyn: Arc<dyn SingleStepExecutor> = executor.clone();
        let ctx = run_ctx(CancelToken::new());
        let mut registry = OutputRegistry::new();
        registry.register("targets", serde_json::json!([{ "path": "x" }, { "path": "x" }]));

        let err = walk_chain(&graph, &mut registry, &executor_dyn, &ctx)
            .await
            .expect_err("duplicate item keys must fail the walk");
        assert!(
            matches!(&err, SubagentError::StructuredOutputInvalid(m) if m.contains("duplicate item key")),
            "got: {err:?}"
        );
    }

    #[tokio::test]
    async fn dynamic_group_colliding_ids_fail_the_walk() {
        let dynamic = dynamic_group_full(
            "outputs.targets",
            single_step("reviewer", "Review {t.path}"),
            "reviews",
            Some("t"),
            Some("/path"),
            Some(4),
            OnEmpty::Skip,
        );
        let graph: ChainGraph = vec![RunnerStep::DynamicGroup(dynamic)];
        let executor = Arc::new(RecordingExecutor::default());
        let executor_dyn: Arc<dyn SingleStepExecutor> = executor.clone();
        let ctx = run_ctx(CancelToken::new());
        let mut registry = OutputRegistry::new();
        // "a/b" and "a-b" are distinct keys that normalize to the same id.
        registry.register("targets", serde_json::json!([{ "path": "a/b" }, { "path": "a-b" }]));

        let err = walk_chain(&graph, &mut registry, &executor_dyn, &ctx)
            .await
            .expect_err("colliding normalized item ids must fail the walk");
        assert!(
            matches!(&err, SubagentError::StructuredOutputInvalid(m) if m.contains("colliding item id")),
            "got: {err:?}"
        );
    }

    #[tokio::test]
    async fn dynamic_group_unknown_template_reference_fails_the_walk() {
        // `{other.path}` names neither the item (`t`) nor a reserved chain reference.
        let dynamic = dynamic_group_full(
            "outputs.targets",
            single_step("reviewer", "Review {other.path}"),
            "reviews",
            Some("t"),
            Some("/path"),
            Some(4),
            OnEmpty::Skip,
        );
        let graph: ChainGraph = vec![RunnerStep::DynamicGroup(dynamic)];
        let executor = Arc::new(RecordingExecutor::default());
        let executor_dyn: Arc<dyn SingleStepExecutor> = executor.clone();
        let ctx = run_ctx(CancelToken::new());
        let mut registry = OutputRegistry::new();
        registry.register("targets", serde_json::json!([{ "path": "a" }]));

        let err = walk_chain(&graph, &mut registry, &executor_dyn, &ctx)
            .await
            .expect_err("an unsupported template reference must fail the walk");
        assert!(
            matches!(&err, SubagentError::StructuredOutputInvalid(m) if m.contains("Unsupported template reference")),
            "got: {err:?}"
        );
        assert!(
            executor.calls.lock().expect("lock").is_empty(),
            "no child may be dispatched when the template is malformed"
        );
    }

    #[tokio::test]
    async fn dynamic_group_bad_template_diagnostic_wins_over_over_limit_and_duplicate() {
        // pi runs `validateDynamicStepShape` (the template check) at the very top of
        // `resolveDynamicFanoutItems` (`dynamic-fanout.ts:217`), BEFORE the maxItems and duplicate-
        // key checks. So when a step has BOTH a malformed template AND an over-limit / duplicate-key
        // array, the template diagnostic is the one that surfaces — this pins that precedence.
        let dynamic = dynamic_group_full(
            "outputs.targets",
            single_step("reviewer", "Review {other.path}"), // unknown, non-reserved reference
            "reviews",
            Some("t"),
            Some("/path"),
            Some(1), // cap below the 2-element array -> also an over-limit error on its own
            OnEmpty::Skip,
        );
        let graph: ChainGraph = vec![RunnerStep::DynamicGroup(dynamic)];
        let executor = Arc::new(RecordingExecutor::default());
        let executor_dyn: Arc<dyn SingleStepExecutor> = executor.clone();
        let ctx = run_ctx(CancelToken::new());
        let mut registry = OutputRegistry::new();
        // Two identical paths: over-limit (2 > 1) AND duplicate key AND colliding id — every
        // item-resolution diagnostic is also live, yet the template error must win.
        registry.register("targets", serde_json::json!([{ "path": "x" }, { "path": "x" }]));

        let err = walk_chain(&graph, &mut registry, &executor_dyn, &ctx)
            .await
            .expect_err("a malformed template must fail the walk");
        assert!(
            matches!(&err, SubagentError::StructuredOutputInvalid(m) if m.contains("Unsupported template reference")),
            "the template diagnostic must win over maxItems/duplicate errors: {err:?}"
        );
        assert!(
            executor.calls.lock().expect("lock").is_empty(),
            "no child may be dispatched"
        );
    }

    #[tokio::test]
    async fn dynamic_group_collect_registers_the_record_shape() {
        let dynamic = dynamic_group_full(
            "outputs.targets",
            single_step("reviewer", "Review {t.path}"),
            "reviews",
            Some("t"),
            Some("/path"),
            Some(4),
            OnEmpty::Skip,
        );
        let graph: ChainGraph = vec![RunnerStep::DynamicGroup(dynamic)];
        let executor = Arc::new(RecordingExecutor::default());
        let executor_dyn: Arc<dyn SingleStepExecutor> = executor.clone();
        let ctx = run_ctx(CancelToken::new());
        let mut registry = OutputRegistry::new();
        registry.register(
            "targets",
            serde_json::json!([{ "path": "src/a.ts" }, { "path": "src/b.ts" }]),
        );

        walk_chain(&graph, &mut registry, &executor_dyn, &ctx)
            .await
            .expect("walk succeeds");

        // The collect output is registered as the ORDERED collect-record array (pi
        // `outputs[collect.as] = { structured: collected }`), not the raw child-structured array.
        let collected = registry
            .get("reviews")
            .and_then(|entry| entry.structured.clone())
            .expect("collect output registered as a structured value");
        let records = collected.as_array().expect("collect output is a JSON array");
        assert_eq!(records.len(), 2);
        assert_eq!(records[0]["key"], serde_json::json!("src/a.ts"));
        assert_eq!(records[0]["index"], serde_json::json!(0));
        assert_eq!(records[0]["item"], serde_json::json!({ "path": "src/a.ts" }));
        assert_eq!(records[0]["agent"], serde_json::json!("reviewer"));
        assert_eq!(records[0]["exitCode"], serde_json::json!(0));
        assert_eq!(records[0]["text"], serde_json::json!("ok"));
        assert_eq!(records[1]["key"], serde_json::json!("src/b.ts"));
        assert_eq!(records[1]["index"], serde_json::json!(1));
    }

    #[tokio::test]
    async fn dynamic_group_on_empty_skip_registers_an_empty_collection() {
        let dynamic = dynamic_group_full(
            "outputs.targets",
            single_step("reviewer", "Review {t.path}"),
            "reviews",
            Some("t"),
            Some("/path"),
            Some(4),
            OnEmpty::Skip,
        );
        let graph: ChainGraph = vec![RunnerStep::DynamicGroup(dynamic)];
        let executor = Arc::new(RecordingExecutor::default());
        let executor_dyn: Arc<dyn SingleStepExecutor> = executor.clone();
        let ctx = run_ctx(CancelToken::new());
        let mut registry = OutputRegistry::new();
        registry.register("targets", serde_json::json!([]));

        let (results, groups) = walk_chain(&graph, &mut registry, &executor_dyn, &ctx)
            .await
            .expect("empty-skip dynamic group succeeds");
        assert_eq!(results.len(), 1);
        assert!(results[0].success);
        assert_eq!(
            groups.len(),
            1,
            "an empty dynamic group still yields exactly one group_results entry (order invariant)"
        );
        assert!(groups[0].children.is_empty());
        assert!(
            executor.calls.lock().expect("lock").is_empty(),
            "no child is dispatched for an empty source array"
        );
        let collected = registry
            .get("reviews")
            .and_then(|entry| entry.structured.clone())
            .expect("empty collection registered");
        assert_eq!(collected, serde_json::json!([]));
    }

    #[tokio::test]
    async fn dynamic_group_on_empty_fail_fails_the_walk() {
        let dynamic = dynamic_group_full(
            "outputs.targets",
            single_step("reviewer", "Review {t.path}"),
            "reviews",
            Some("t"),
            Some("/path"),
            Some(4),
            OnEmpty::Fail,
        );
        let graph: ChainGraph = vec![RunnerStep::DynamicGroup(dynamic)];
        let executor = Arc::new(RecordingExecutor::default());
        let executor_dyn: Arc<dyn SingleStepExecutor> = executor.clone();
        let ctx = run_ctx(CancelToken::new());
        let mut registry = OutputRegistry::new();
        registry.register("targets", serde_json::json!([]));

        let err = walk_chain(&graph, &mut registry, &executor_dyn, &ctx)
            .await
            .expect_err("onEmpty=fail must fail the walk on an empty source");
        assert!(
            matches!(&err, SubagentError::StructuredOutputInvalid(m) if m.contains("source array is empty")),
            "got: {err:?}"
        );
    }

    #[tokio::test]
    async fn dynamic_group_index_keys_when_no_expand_key() {
        // With no expand.key, the item key is the stringified index (pi `String(index)`).
        let dynamic = dynamic_group_full(
            "outputs.targets",
            single_step("reviewer", "handle one item"),
            "reviews",
            None,
            None,
            Some(4),
            OnEmpty::Skip,
        );
        let graph: ChainGraph = vec![RunnerStep::DynamicGroup(dynamic)];
        let executor = Arc::new(RecordingExecutor::default());
        let executor_dyn: Arc<dyn SingleStepExecutor> = executor.clone();
        let ctx = run_ctx(CancelToken::new());
        let mut registry = OutputRegistry::new();
        registry.register("targets", serde_json::json!([{ "id": 1 }, { "id": 2 }]));

        walk_chain(&graph, &mut registry, &executor_dyn, &ctx)
            .await
            .expect("walk succeeds");
        let collected = registry
            .get("reviews")
            .and_then(|entry| entry.structured.clone())
            .expect("collect registered");
        let records = collected.as_array().expect("array");
        assert_eq!(records[0]["key"], serde_json::json!("0"));
        assert_eq!(records[1]["key"], serde_json::json!("1"));
    }

    // ---- Serialization round-trip: tagged JSON per arch-SA §4.2 ----

    #[test]
    fn runner_step_round_trips_through_tagged_json_for_all_three_shapes() {
        let steps: ChainGraph = vec![
            RunnerStep::SingleStep(single_step("a", "t")),
            RunnerStep::ParallelGroup(ParallelGroupSpec {
                steps: vec![single_step("b", "t2")],
                concurrency: 1,
                fail_fast: true,
                worktree: false,
            }),
            RunnerStep::DynamicGroup(DynamicGroupSpec {
                expand: "outputs.x".to_string(),
                template: Box::new(single_step("c", "t3")),
                collect: "y".to_string(),
                concurrency: 2,
                item: Some("it".to_string()),
                key: Some("/id".to_string()),
                max_items: Some(5),
                on_empty: OnEmpty::Fail,
                collect_schema: Some(serde_json::json!({ "type": "array" })),
            }),
        ];
        let json = serde_json::to_string(&steps).expect("serializes");
        let round_tripped: ChainGraph = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(steps, round_tripped);
    }

    // ---- Basic ordering sanity for the OutputRegistry itself ----

    #[test]
    fn output_registry_resolve_handles_multiple_references_in_one_template() {
        // Plain-text outputs (C11): their `.text` is substituted verbatim, so a repeated reference
        // resolves consistently.
        let mut registry = OutputRegistry::new();
        registry.register_text("a", "A");
        registry.register_text("b", "B");
        assert_eq!(
            registry
                .resolve("{outputs.a}-{outputs.b}-{outputs.a}")
                .expect("all names registered"),
            "A-B-A"
        );
    }

    #[test]
    fn output_registry_resolve_renders_structured_values_as_compact_json() {
        // A structured output's `{outputs.name}` substitution is its compact JSON encoding (pi's
        // `compactStructuredText` = `JSON.stringify`) — a bare number renders as `42`.
        let mut registry = OutputRegistry::new();
        registry.register("n", serde_json::json!(42));
        assert_eq!(
            registry.resolve("value: {outputs.n}").expect("registered"),
            "value: 42"
        );
        // An object renders as compact JSON, and a structured string renders QUOTED (JSON.stringify),
        // distinguishing structured from plain-text registration.
        registry.register("obj", serde_json::json!({ "k": 1 }));
        registry.register("s", serde_json::json!("hi"));
        assert_eq!(
            registry.resolve("{outputs.obj} {outputs.s}").expect("registered"),
            "{\"k\":1} \"hi\""
        );
    }

    /// A structured-output-producing step increments a shared counter observably once — a cheap
    /// proxy proving [`walk_chain`] does not call `run_single` more than once per `SingleStep`
    /// list element (no accidental retry/duplication baked into the walker itself; retry ladders
    /// belong to a later phase's `exec::run_sync`, not this file).
    #[tokio::test]
    async fn each_single_step_is_dispatched_exactly_once() {
        struct CountingExecutor {
            calls: AtomicUsize,
        }
        #[async_trait::async_trait]
        impl SingleStepExecutor for CountingExecutor {
            async fn run_single(
                &self,
                _step: &SingleStepSpec,
                _resolved_task: &str,
                _ctx: &ChainRunContext,
            ) -> Result<StepResult, SubagentError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok(StepResult::success(None, None))
            }
        }

        let executor = Arc::new(CountingExecutor {
            calls: AtomicUsize::new(0),
        });
        let graph: ChainGraph = vec![
            RunnerStep::SingleStep(single_step("a", "1")),
            RunnerStep::SingleStep(single_step("b", "2")),
        ];
        let cancel = CancelToken::new();
        let ctx = run_ctx(cancel);
        let mut registry = OutputRegistry::new();

        let executor_dyn: Arc<dyn SingleStepExecutor> = executor.clone();
        walk_chain(&graph, &mut registry, &executor_dyn, &ctx)
            .await
            .expect("walk succeeds");

        assert_eq!(executor.calls.load(Ordering::SeqCst), 2);
    }
}
