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
use std::path::PathBuf;
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
    /// [`OutputRegistry`], if any (func-SA §4.2 `output`). Absent means this step's result is not
    /// referenceable by any later step's `{outputs.name}` reference.
    pub output: Option<String>,
    /// Where/how this step's final output is written (func-SA §4.2 `outputMode`).
    pub output_mode: Option<OutputMode>,
    /// Pre-declared read-context paths for this step (func-SA §4.2 `reads`).
    pub reads: Option<Vec<PathBuf>>,
    /// Explicit acceptance-contract override for this step (func-SA §4.2 `acceptance`); `None`
    /// defers to the agent's own default / heuristic inference (R-SA-023).
    pub acceptance: Option<String>,
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
    /// The per-item step template. One concrete [`SingleStepSpec`] is materialized per element of
    /// the array [`DynamicGroupSpec::expand`] resolves to; template-parameter substitution
    /// (mapping each array element's fields into `template`'s own `task`/other string fields) is
    /// intentionally minimal here — see [`walk_chain`]'s doc note on template instantiation.
    pub template: Box<SingleStepSpec>,
    /// Named-output key the *collected* (fanned-in) array of per-item results is registered
    /// under in the chain-wide [`OutputRegistry`], analogous to [`SingleStepSpec::output`] but
    /// for the group's aggregate result rather than one step's own result.
    pub collect: String,
    /// Local worker-pool concurrency ceiling for the expanded group, identical in meaning to
    /// [`ParallelGroupSpec::concurrency`].
    pub concurrency: u32,
}

/// The discriminated union `SingleStep | ParallelGroup | DynamicGroup` (func-SA §4.2). Tagged
/// JSON so chain files (`.chain.json`/`.chain.md`) and the one-shot runner-config hand-off file
/// (arch-SA §4.3) can (de)serialize it directly.
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
}

/// `ChainGraph` is nothing more than an ordered list of [`RunnerStep`]s, walked strictly in order
/// (R-SA-052). This type alias exists purely for readability at call sites — it carries no
/// additional structure, no node ids, and no edges beyond "comes before"/"comes after" in the
/// `Vec`'s own order.
pub type ChainGraph = Vec<RunnerStep>;

// -------------------------------------------------------------------------------------------
// OutputRegistry: named-output cross-step data dependencies (R-SA-053)
// -------------------------------------------------------------------------------------------

/// The append-only accumulator of named step outputs [`walk_chain`] builds up as it proceeds
/// through a [`ChainGraph`] (R-SA-053). A step being evaluated can only ever resolve references
/// against outputs already registered by strictly earlier steps — there is no API on this type
/// that can observe an output not yet inserted, so "strictly earlier only" is structural, not
/// merely a convention this type's callers must remember to uphold.
#[derive(Debug, Default, Clone)]
pub struct OutputRegistry {
    outputs: BTreeMap<String, Value>,
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
    pub fn register(&mut self, name: impl Into<String>, value: Value) {
        self.outputs.insert(name.into(), value);
    }

    /// Fetch a strictly-earlier step's whole registered output by name, or `None` if no step has
    /// registered that name yet (either because no step ever will, or — from the walker's own
    /// point of view while mid-chain — because the producing step has not run yet, which by
    /// construction cannot happen for a well-formed `{outputs.name}` reference resolved at its
    /// own step's dispatch time, since chain authoring only ever references earlier steps).
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.outputs.get(name)
    }

    /// Resolve every `{outputs.name}` reference embedded in `template` by literal substring
    /// substitution against this registry's currently-registered outputs (R-SA-053). Every
    /// `{outputs.<name>}` occurrence is replaced with `<name>`'s registered value rendered as a
    /// plain string (a JSON string value is unwrapped to its bare text; any other JSON value type
    /// is rendered via its compact JSON text form) — an occurrence naming a not-yet-registered
    /// output is left untouched in the returned string rather than erroring, since a reference to
    /// a step that genuinely never produced a named output is a chain-authoring error better
    /// surfaced by whatever consumes the still-unresolved placeholder downstream (this function's
    /// job is purely mechanical substitution, not chain validation).
    #[must_use]
    pub fn resolve(&self, template: &str) -> String {
        let mut result = String::with_capacity(template.len());
        let mut rest = template;
        while let Some(start) = rest.find("{outputs.") {
            result.push_str(&rest[..start]);
            let after_prefix = &rest[start + "{outputs.".len()..];
            let Some(end) = after_prefix.find('}') else {
                // No closing brace at all for the remainder of the string: emit the rest
                // verbatim and stop scanning (nothing left that could be a well-formed
                // reference).
                result.push_str(&rest[start..]);
                rest = "";
                break;
            };
            let name = &after_prefix[..end];
            match self.outputs.get(name) {
                Some(Value::String(s)) => result.push_str(s),
                Some(other) => result.push_str(&other.to_string()),
                None => {
                    // Not (yet) registered: leave the original placeholder text untouched.
                    result.push_str("{outputs.");
                    result.push_str(name);
                    result.push('}');
                }
            }
            rest = &after_prefix[end + 1..];
        }
        result.push_str(rest);
        result
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

        let mut current = self.outputs.get(name).ok_or_else(|| {
            SubagentError::StructuredOutputInvalid(format!(
                "expand pointer references output \"{name}\", which is not a strictly-earlier \
                 step's registered structured output"
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
/// # Template instantiation for `DynamicGroup`
///
/// `template`'s per-item field substitution (mapping each resolved array element's own fields
/// into the template's `task`/other string fields, beyond the flat `{outputs.name}` substitution
/// [`OutputRegistry::resolve`] already performs) is intentionally minimal in this function: given
/// the current one-`template`-value-per-array-element contract, [`walk_chain`] instantiates
/// `items.len()` clones of `template` (with only the flat `{outputs.name}` substitution already
/// applied to `task`) as the group to delegate to `run_bounded`. A real per-item
/// template-parameter binding (e.g. an `{item.field}` placeholder syntax reaching into each
/// array element's own fields) is explicitly deferred to a later phase of this crate's build-out
/// (the foreground executor / chain-authoring surface, `exec/mod.rs` or `discovery/chains.rs`'s
/// own future extension) and is noted here rather than silently assumed solved — R-SA-053's own
/// text scopes THIS file's responsibility to "`DynamicGroup.expand` MUST resolve its source array
/// ... only", which is fully implemented; per-item template binding is a distinct, unaddressed
/// concern this comment flags explicitly rather than leaving unstated.
pub async fn walk_chain(
    graph: &ChainGraph,
    registry: &mut OutputRegistry,
    single: &Arc<dyn SingleStepExecutor>,
    ctx: &ChainRunContext,
) -> Result<(Vec<StepResult>, Vec<GroupStepResult>), SubagentError> {
    let mut results = Vec::with_capacity(graph.len());
    let mut group_results = Vec::new();

    for step in graph {
        let result = match step {
            RunnerStep::SingleStep(spec) => {
                let resolved_task = registry.resolve(&spec.task);
                let result = single
                    .run_single(spec, &resolved_task, ctx)
                    .await
                    .unwrap_or_else(|err| StepResult::failure(err.to_string()));
                if let Some(name) = &spec.output
                    && let Some(value) = &result.structured_output
                {
                    registry.register(name.clone(), value.clone());
                }
                result
            }
            RunnerStep::ParallelGroup(spec) => {
                let mut resolved_steps: Vec<SingleStepSpec> = spec
                    .steps
                    .iter()
                    .map(|s| SingleStepSpec {
                        task: registry.resolve(&s.task),
                        ..s.clone()
                    })
                    .collect();

                if spec.worktree {
                    assign_worktree_cwds(&mut resolved_steps, ctx).await?;
                }

                let group_result =
                    dispatch_group(resolved_steps, spec.concurrency, spec.fail_fast, single, ctx)
                        .await;
                let collapsed = group_result.aggregate.clone();
                group_results.push(group_result);
                collapsed
            }
            RunnerStep::DynamicGroup(spec) => {
                let items = registry.resolve_pointer(&spec.expand)?;
                let resolved_template_task = registry.resolve(&spec.template.task);
                let expanded: Vec<SingleStepSpec> = items
                    .iter()
                    .map(|_| SingleStepSpec {
                        task: resolved_template_task.clone(),
                        ..(*spec.template).clone()
                    })
                    .collect();

                let group_result =
                    dispatch_group(expanded, spec.concurrency, false, single, ctx).await;
                let collapsed = group_result.aggregate.clone();
                if let Some(value) = &collapsed.structured_output {
                    registry.register(spec.collect.clone(), value.clone());
                }
                group_results.push(group_result);
                collapsed
            }
        };
        results.push(result);
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

    GroupStepResult {
        aggregate: StepResult {
            success,
            structured_output: Some(structured_output),
            final_output: None,
            error,
        },
        children,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

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
            cancel,
            global_limit: GlobalConcurrencyLimit::default_limit(),
            worktree_base_dir: None,
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
    /// single-step — proving both linear order and that `{outputs.name}` resolution happened
    /// before dispatch.
    #[derive(Default)]
    struct RecordingExecutor {
        calls: StdMutex<Vec<String>>,
        structured_output_for: StdMutex<std::collections::HashMap<String, Value>>,
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
            let structured_output = self
                .structured_output_for
                .lock()
                .expect("lock")
                .get(resolved_task)
                .cloned();
            Ok(StepResult::success(
                Some("ok".to_string()),
                structured_output,
            ))
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
        assert_eq!(
            *executor.calls.lock().expect("lock"),
            vec![
                "first".to_string(),
                "second".to_string(),
                "third".to_string()
            ],
            "steps must be dispatched in exactly the order they appear in the chain graph"
        );
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
        let step3_pos = calls.iter().position(|c| c == "step-3").expect("present");
        let p1_pos = calls.iter().position(|c| c == "p-1").expect("present");
        let p2_pos = calls.iter().position(|c| c == "p-2").expect("present");
        assert!(step3_pos > p1_pos && step3_pos > p2_pos);
        assert_eq!(calls[0], "step-1");
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

    // ---- R-SA-053: named-output resolution against strictly earlier steps only ----

    #[tokio::test]
    async fn a_later_step_resolves_an_earlier_steps_named_output() {
        let mut first = single_step("researcher", "find the answer");
        first.output = Some("finding".to_string());
        let second = single_step("writer", "write about {outputs.finding}");
        let graph: ChainGraph = vec![
            RunnerStep::SingleStep(first),
            RunnerStep::SingleStep(second),
        ];
        let executor = Arc::new(RecordingExecutor::default());
        let executor_dyn: Arc<dyn SingleStepExecutor> = executor.clone();
        executor.structured_output_for.lock().expect("lock").insert(
            "find the answer".to_string(),
            Value::String("42".to_string()),
        );
        let cancel = CancelToken::new();
        let ctx = run_ctx(cancel);
        let mut registry = OutputRegistry::new();

        walk_chain(&graph, &mut registry, &executor_dyn, &ctx)
            .await
            .expect("walk succeeds");

        let calls = executor.calls.lock().expect("lock").clone();
        assert_eq!(calls[0], "find the answer");
        assert_eq!(
            calls[1], "write about 42",
            "the second step's {{outputs.finding}} reference must resolve to the first step's \
             registered structured output"
        );
    }

    #[tokio::test]
    async fn an_unregistered_output_reference_is_left_unresolved_not_erroring() {
        let registry = OutputRegistry::new();
        let resolved = registry.resolve("use {outputs.never_registered} here");
        assert_eq!(
            resolved, "use {outputs.never_registered} here",
            "a reference to a name nothing has registered must pass through untouched"
        );
    }

    #[tokio::test]
    async fn output_registered_by_step_one_is_not_visible_before_step_one_completes() {
        // Structural proof of "strictly earlier only": resolve() against a registry that has not
        // yet had step one's output registered must not find it, even though the SAME name will
        // be registered moments later — there is no way to reach it early.
        let registry = OutputRegistry::new();
        assert_eq!(registry.get("finding"), None);
        assert_eq!(registry.resolve("{outputs.finding}"), "{outputs.finding}");
    }

    // ---- DynamicGroup: expand resolves only against validated structured output ----

    #[tokio::test]
    async fn dynamic_group_expands_over_a_prior_steps_structured_output_array() {
        let mut source = single_step("planner", "make a plan");
        source.output = Some("plan".to_string());
        let dynamic = DynamicGroupSpec {
            expand: "outputs.plan".to_string(),
            template: Box::new(single_step("worker", "handle one item")),
            collect: "results".to_string(),
            concurrency: 4,
        };
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
        let dynamic = DynamicGroupSpec {
            expand: "outputs.plan/items".to_string(),
            template: Box::new(single_step("worker", "handle one item")),
            collect: "results".to_string(),
            concurrency: 2,
        };
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
        let dynamic = DynamicGroupSpec {
            expand: "outputs.never_produced".to_string(),
            template: Box::new(single_step("worker", "handle one item")),
            collect: "results".to_string(),
            concurrency: 2,
        };
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
        let dynamic = DynamicGroupSpec {
            expand: "outputs.plan".to_string(),
            template: Box::new(single_step("worker", "handle one item")),
            collect: "results".to_string(),
            concurrency: 2,
        };
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
            }),
        ];
        let json = serde_json::to_string(&steps).expect("serializes");
        let round_tripped: ChainGraph = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(steps, round_tripped);
    }

    // ---- Basic ordering sanity for the OutputRegistry itself ----

    #[test]
    fn output_registry_resolve_handles_multiple_references_in_one_template() {
        let mut registry = OutputRegistry::new();
        registry.register("a", Value::String("A".to_string()));
        registry.register("b", Value::String("B".to_string()));
        assert_eq!(
            registry.resolve("{outputs.a}-{outputs.b}-{outputs.a}"),
            "A-B-A"
        );
    }

    #[test]
    fn output_registry_resolve_renders_non_string_values_as_json() {
        let mut registry = OutputRegistry::new();
        registry.register("n", serde_json::json!(42));
        assert_eq!(registry.resolve("value: {outputs.n}"), "value: 42");
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
