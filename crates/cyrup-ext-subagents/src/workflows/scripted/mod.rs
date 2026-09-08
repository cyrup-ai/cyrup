//! The agent-authored workflow runtime — pi `workflows/scripted-workflow.ts` (2,284 LOC) ported
//! onto **V8, embedded through `deno_core`** (SCOPE_3f). Facade + narrative only; no logic.
//!
//! # What this is for
//!
//! An agent needs to run other agents and choose what to run next based on what came back. The
//! static `chain`/`parallel` plan cannot branch on a result; branching in the orchestrator's own
//! loop costs a full inference round-trip AND a context payload per branch. `workflowScript` lets
//! the agent write the plan ONCE — plain JavaScript, top-level await, `runs.*` — and the plan
//! executes without further inference. Upstream states the thesis in the tool prompt itself
//! (`tool-description.ts:19`): *"make exactly one top-level subagent call … launch children only
//! inside that workflow"*.
//!
//! The author is **a model filling in a tool-call argument**, which fixes three constraints
//! (§0.1): the language is JavaScript because of prior mass (any bespoke surface trades a
//! 10⁹-line prior for a 0-line prior); conformance is the dominant correctness term; and errors
//! are re-prompts, so **every error string in this module family is a product surface, ported
//! byte-for-byte**.
//!
//! # The capability contract — measured, not assumed (§0.3)
//!
//! Upstream's sandbox is `vm.createContext({ runs, Promise, emit, console }, { codeGeneration: {
//! strings: false, wasm: false } })` (`:914-916`) — a bare V8 context. Grepping upstream for any
//! host-API injection into that sandbox returns nothing. So the contract is exactly:
//!
//! | available | NOT available |
//! |---|---|
//! | `runs`, `emit`, `console`, `state` (only with a mission) | `setTimeout`, `fetch`, `require`, `Buffer`, `process` |
//! | every ECMAScript built-in | `TextEncoder`, `URL`, `crypto`, `structuredClone`, `queueMicrotask` |
//! | top-level `await`, `Promise` chains | `eval` / `new Function`; nested `async` functions |
//!
//! and the prompt says so verbatim: *"Available globals are runs…, emit, console, and standard
//! JavaScript only… Scripts cannot access filesystem, shell, arbitrary Pi tools, or host globals."*
//!
//! **Timers are excluded by design, not by limitation.** `deno_core` would give us `setTimeout` if
//! we asked; the global lives in `deno_web`/`deno_runtime`, which is deliberately not a dependency.
//! A workflow's waits are waits on *work* — every `runs.*` call blocks on a real event and so
//! appears in the trace, on the live card, and is cancellable. A timer is a wait on nothing:
//! invisible to the trace while still spending the run budget. A timer in a workflow is also
//! almost always a reconstructed poll loop, which is the exact thing this feature deletes.
//! `runs.sleep(ms)` is therefore **rejected** and must not be re-proposed: it would make the trace
//! lie about what the run was doing. Because the contract is a closed set, [`analyzer`] enforces it
//! at `action:"validate"` time — a script reaching for `setTimeout` is rejected with a re-prompt
//! *before a child is spent*, where upstream discovers it as a runtime `ReferenceError` after the
//! fact.
//!
//! # Why `deno_core`, and why not the wasm component that was here first
//!
//! "Conformance" is three different things, and conflating them is what produced the design this
//! replaces (§0.2): **(a)** spec conformance (test262) is a headline number, not a requirement;
//! **(b)** *prior* conformance — does the engine behave the way the authoring model predicts, whose
//! reference implementation is V8-in-Node — is the requirement; **(c)** library conformance (can it
//! run acorn and a large prelude) was self-inflicted and is designed away by keeping the prelude
//! outside the sandboxed script, exactly as upstream's split at `:996` does.
//!
//! `deno_core` satisfies (b) *by construction: it is V8*. The prior design used StarlingMonkey and
//! paid for it with a 25 MB unreproducible wasm blob committed to git, a Node/npm build chain
//! (~350 MB of `node_modules`) that could not run on the dev machine at all, an explicitly
//! experimental packaging tool, and a whole class of deadlocks created by working around Component
//! Model async — a suspending host import serialises the instance, so fan-out needed a handle/poll
//! protocol. Here ops are natively async on tokio and `runs.all` fans out for free (§3.2).
//!
//! The trade was never "pure Rust vs C++": it was a checksummed cargo dependency versus an
//! unreproducible binary artifact.
//!
//! # The C++ ruling (§1.3) — recorded here because it will be challenged
//!
//! V8 is C++. The root `Cargo.toml` rule is *"no C dependency **where a pure-Rust alternative
//! exists**"* — conditional, not absolute — and the workspace has already exercised the carve-out
//! four times: `aws-lc-sys`, `ring`, `tree-sitter`, `zstd-sys`. V8 is admitted on the same grounds:
//! the pure-Rust alternative (`boa_engine`) fails bar (b), which is the term the entire feature
//! rests on. `forbid(unsafe_code)` is a per-crate lint and is unaffected transitively — the same
//! reasoning already recorded for `syntect`.
//!
//! `rusty_v8`'s build downloads a **checksum-verified** prebuilt static library; `V8_FROM_SOURCE=1`
//! builds from source and `RUSTY_V8_MIRROR` points the download at an internal mirror for closed
//! CI. Nothing is vendored into git.
//!
//! `docs/gap-analysis/MCP-PORT-METHODOLOGY.md:1382`'s guard
//! (`! rg -qi 'rquickjs|boa_engine|deno_core|v8' crates/cyrup-mcp/Cargo.toml`) is cited accurately:
//! it is scoped to **`crates/cyrup-mcp/Cargo.toml`** and its own rationale at `:67` is *"this
//! removes the only JS-engine question in the port"* — it guards against re-adding an engine for
//! `mcpScript`, a feature that was **cut**. Here the JS feature is the deliverable. That guard is
//! left exactly as written.
//!
//! # Module map
//!
//! ```text
//! mod.rs        facade + the contract, the ruling, and the rejection record. No logic.
//! engine.rs     the run/validate entry points, the orchestration state, the isolate lifecycle
//! ops.rs        the #[op2] layer — the whole capability surface (§3.2)
//! analyzer.rs   deno_ast structural analysis: nested-async + enumerated globals (§3.6)
//! js/prelude.js the kept half of upstream's WORKER_SOURCE, as source (§3.4)
//! recovery.rs   the acceptance-recovery review classifier (:1150-1320)
//! git_ref.rs    valid_git_ref + BASE_REF_VALIDATION_ERROR
//! permit.rs     WorkflowChildPermit + its three-state lifecycle
//! json_value.rs assert_workflow_json_value
//! preview.rs    preview_simple_workflow_run (:1370-1386)
//! settlement.rs the one pure unawaited-work precedence decision
//! types.rs      the eleven public types (:997-1101)
//! ```

mod analyzer;
mod engine;
mod git_ref;
mod json_value;
mod ops;
mod permit;
mod preview;
mod recovery;
mod settlement;
mod types;

pub use analyzer::{
    AnalysisReport, AnalyzerOptions, NESTED_ASYNC_VALIDATION_ERROR, NESTED_ASYNC_WORKFLOW_ERROR,
    analyze_workflow_script,
};
pub use engine::{
    NESTED_WORKFLOW_REFUSAL, RunWorkflowScriptOptions, WORKFLOW_ASSEMBLY_FLUSH_TIMEOUT_MS,
    WORKFLOW_CHILD_MARKER, WORKFLOW_DEFAULT_TIMEOUT_MS, refuse_nested_workflow,
    WORKFLOW_SETTLE_DRAIN_TIMEOUT_MS, WorkflowEmitCallback, WorkflowHostStepCallback,
    WorkflowLanePlanCallback, WorkflowLaunchAdmission, WorkflowPermitClaim, WorkflowResolvedResume,
    WorkflowResumeInput, WorkflowRunCall, WorkflowScriptHost, WorkflowStateStore,
    WorkflowStopChild, WorkflowTraceCallback, run_workflow_script, validate_workflow_script,
};
pub use ops::ObservationKind;
pub use git_ref::{BASE_REF_VALIDATION_ERROR, valid_git_ref};
pub use json_value::{assert_workflow_json_value, format_workflow_json_preview};
pub use permit::{
    WorkflowChildContext, WorkflowChildPermit, WorkflowChildPermitError,
    WorkflowChildPermitInput, WorkflowChildPermitInputError, WorkflowChildPermitLaunch,
};
pub use preview::{SimpleWorkflowRunPreview, preview_simple_workflow_run};
pub use recovery::{
    is_acceptance_metadata_recovery, is_explicit_read_only_recovery_review,
    recovery_barrier_message,
};
pub use settlement::CompletionSettlement;
pub use types::{
    WorkflowConsoleEntry, WorkflowConsoleLevel, WorkflowLanePlan, WorkflowLanePlanStage,
    WorkflowReceiptResumeReference, WorkflowResolvedResumeReference, WorkflowScriptError,
    WorkflowScriptErrorKind, WorkflowScriptPartial, WorkflowScriptResult,
    WorkflowScriptValidationError, WorkflowScriptValidationResult, WorkflowSteerMode,
    WorkflowSteerOptions, WorkflowSteerResult, WorkflowSteerState, WorkflowSteerTarget,
};
