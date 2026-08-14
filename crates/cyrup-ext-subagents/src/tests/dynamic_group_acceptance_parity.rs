//! SUBA-C14 — a dynamic-fanout step's own GROUP-level `acceptance` gate must actually be evaluated.
//!
//! `acceptance` is a legal dynamic-step key at the ported baseline
//! (`pi-subagents:v0.34.0:src/runs/shared/dynamic-fanout.ts:45` `DYNAMIC_STEP_KEYS`) and
//! `src/runs/foreground/chain-execution.ts` evaluates it once the group settles — at `:1034-1055`
//! for a completed group and at `:869-891` for an empty fan-out — building the report with
//! `aggregateAcceptanceReport({ results, notes })`, running `evaluateAcceptance`, and returning
//! `buildChainExecutionErrorResult(acceptanceFailureMessage(...))` on rejection, which fails the
//! WHOLE chain.
//!
//! Before this change `discovery/chains.rs` listed `"acceptance"` in its own `DYNAMIC_STEP_KEYS`
//! and shape-checked it at parse time, and then `chain_step_to_runner_step` dropped it:
//! `DynamicGroupSpec` had no `acceptance` field at all and `walk_chain`'s `DynamicGroup` arm ran no
//! gate of any kind. A declared group gate was accepted as legal and silently ignored, so a chain
//! pi fails reported success.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::Arc;

use cyrup_core::CancelToken;
use crate::error::SubagentError;
use crate::spawn::chain_graph::{
    ChainRunContext, DynamicGroupSpec, OnEmpty, OutputRegistry, RunnerStep, SingleStepExecutor,
    SingleStepSpec, StepResult, walk_chain,
};
use crate::spawn::parallel::GlobalConcurrencyLimit;

/// A [`SingleStepExecutor`] that succeeds every child with a fixed final output. The walker's
/// acceptance gate is a pure function of the group's settled outcome, so a child that simply
/// succeeds is all this test needs — the REAL subprocess mechanism is exercised by
/// `spawn/`'s own tests and is not what is under test here.
struct AlwaysSucceeds;

#[async_trait::async_trait]
impl SingleStepExecutor for AlwaysSucceeds {
    async fn run_single(
        &self,
        _step: &SingleStepSpec,
        resolved_task: &str,
        _ctx: &ChainRunContext,
    ) -> Result<StepResult, SubagentError> {
        Ok(StepResult::success(
            Some(format!("done: {resolved_task}")),
            None,
        ))
    }
}

fn template(agent: &str, task: &str) -> SingleStepSpec {
    SingleStepSpec {
        agent: agent.to_string(),
        task: task.to_string(),
        skills: None,
        session_dir: None,
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

fn dynamic_step(acceptance: Option<serde_json::Value>) -> DynamicGroupSpec {
    DynamicGroupSpec {
        expand: "outputs.targets".to_string(),
        template: Box::new(template("worker", "Review {item.id}")),
        collect: "reviews".to_string(),
        concurrency: 2,
        fail_fast: false,
        item: None,
        key: None,
        max_items: Some(8),
        on_empty: OnEmpty::Skip,
        collect_schema: None,
        acceptance,
    }
}

fn run_ctx() -> ChainRunContext {
    ChainRunContext {
        cwd: std::env::temp_dir(),
        deadline_at: None,
        timeout_ms: None,
        cancel: CancelToken::new(),
        global_limit: GlobalConcurrencyLimit::default_limit(),
        worktree_base_dir: None,
        original_task: "Review everything".to_string(),
        chain_dir: None,
        dynamic_fanout_max_items: None,
    }
}

/// Drive one dynamic step over `source` and return its step results.
async fn walk(
    acceptance: Option<serde_json::Value>,
    source: serde_json::Value,
) -> Vec<StepResult> {
    let graph = vec![RunnerStep::DynamicGroup(dynamic_step(acceptance))];
    let executor: Arc<dyn SingleStepExecutor> = Arc::new(AlwaysSucceeds);
    let mut registry = OutputRegistry::new();
    registry.register("targets", source);
    let (results, _groups) = walk_chain(&graph, &mut registry, &executor, &run_ctx())
        .await
        .expect("the walk itself must not error — the gate rejects via a failed StepResult");
    results
}

/// The backlog item's own example: a group gate declaring a criterion the aggregate report can
/// never carry. `aggregateAcceptanceReport` reports only `criterion-1`, `criterion-2` and one
/// `child-<n>` per child (`acceptance.ts:1000-1030`), so a criterion named `c1` is simply "not
/// reported" and `checkCriteriaSatisfied` fails it — under pi the whole chain fails.
#[tokio::test]
async fn a_declared_group_gate_that_cannot_be_satisfied_fails_the_chain() {
    let results = walk(
        Some(serde_json::json!({
            "level": "checked",
            "criteria": [{ "id": "c1", "must": "every reviewer signed off" }]
        })),
        serde_json::json!([{ "id": "a" }, { "id": "b" }]),
    )
    .await;

    assert_eq!(results.len(), 1, "one dynamic step, one result");
    assert!(
        !results[0].success,
        "the group-level acceptance gate must fail the step: {:?}",
        results[0]
    );
    let error = results[0].error.clone().unwrap_or_default();
    // pi `acceptanceFailureMessage` (`acceptance.ts:1357-1365`) prefixes the first failed runtime
    // check with `Acceptance rejected: `.
    assert!(
        error.contains("Acceptance rejected: Required criterion 'c1' was not reported."),
        "expected pi's verbatim group-rejection message, got: {error}"
    );
}

/// The same gate, but declaring a criterion the aggregate report DOES satisfy — the group passes
/// and the chain continues, so the gate is a real evaluation rather than a blanket rejection.
#[tokio::test]
async fn a_declared_group_gate_the_aggregate_report_satisfies_lets_the_chain_through() {
    let results = walk(
        Some(serde_json::json!({
            "level": "checked",
            "criteria": ["every dynamic child completed without blockers"]
        })),
        serde_json::json!([{ "id": "a" }, { "id": "b" }]),
    )
    .await;

    assert_eq!(results.len(), 1);
    assert!(
        results[0].success,
        "a bare-string criterion normalizes to `criterion-1`, which the aggregate report reports \
         satisfied when every child succeeded: {:?}",
        results[0]
    );
}

/// The empty-fan-out path runs the gate too (`chain-execution.ts:869-891`), over
/// `aggregateAcceptanceReport({ results: [], notes: "Dynamic fanout produced 0 results." })` —
/// which reports every criterion `not-satisfied`, so a declared gate rejects. This is precisely the
/// case an author declares a gate to catch: `onEmpty: "skip"` otherwise reports success.
#[tokio::test]
async fn an_empty_fanout_with_a_declared_group_gate_still_fails_the_chain() {
    let results = walk(
        Some(serde_json::json!({
            "level": "checked",
            "criteria": ["every dynamic child completed without blockers"]
        })),
        serde_json::json!([]),
    )
    .await;

    assert_eq!(results.len(), 1);
    assert!(
        !results[0].success,
        "a zero-result fan-out satisfies no criterion: {:?}",
        results[0]
    );
    assert!(
        results[0]
            .error
            .clone()
            .unwrap_or_default()
            .contains("Acceptance rejected:"),
        "expected pi's group-rejection message on the empty path, got: {:?}",
        results[0].error
    );
}

/// No declared gate — the pre-existing behavior is unchanged (and the empty fan-out still reports
/// pi's sentinel success). Guards against the gate becoming an unconditional tax on every dynamic
/// step.
#[tokio::test]
async fn a_dynamic_step_with_no_declared_gate_is_unaffected() {
    let completed = walk(None, serde_json::json!([{ "id": "a" }])).await;
    assert!(completed[0].success, "{completed:?}");

    let empty = walk(None, serde_json::json!([])).await;
    assert!(empty[0].success, "{empty:?}");
    assert_eq!(
        empty[0].final_output.as_deref(),
        Some("Dynamic fanout produced 0 results.")
    );
}

/// The gate is carried from the saved chain file, not only from a hand-built graph:
/// `chain_step_to_runner_step` must lower the dynamic step's own `acceptance` onto the spec.
#[test]
fn chain_step_to_runner_step_carries_the_dynamic_steps_own_acceptance() {
    use crate::discovery::chains::chain_step_to_runner_step;
    use crate::discovery::types::ChainStepConfig;

    let policy = serde_json::json!({
        "level": "checked",
        "criteria": [{ "id": "c1", "must": "every reviewer signed off" }]
    });
    let step = ChainStepConfig {
        expand: Some(serde_json::json!({ "from": { "output": "targets" } })),
        parallel: Some(serde_json::json!({ "agent": "worker", "task": "Review {item.id}" })),
        collect: Some(serde_json::json!({ "as": "reviews" })),
        acceptance: Some(policy.clone()),
        ..ChainStepConfig::default()
    };

    match chain_step_to_runner_step(&step, 4) {
        RunnerStep::DynamicGroup(spec) => {
            assert_eq!(
                spec.acceptance,
                Some(policy),
                "the group gate must survive onto the runtime spec"
            );
            assert_eq!(
                spec.template.acceptance, None,
                "the per-item template gate is a separate field and was not declared here"
            );
        }
        other => panic!("expected a DynamicGroup, got {other:?}"),
    }
}
