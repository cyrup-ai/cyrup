//! A dynamic fan-out's collect records must carry the CHILD's real per-run detail, not a
//! success/failure re-derivation.
//!
//! Upstream, `collectDynamicResults`
//! (`pi-subagents:v0.34.0:src/runs/shared/dynamic-fanout.ts:263-287`) copies the child's own
//! `exitCode` verbatim (`:278` — `exitCode: result?.exitCode ?? null`) and conditionally spreads
//! `timedOut` / `outputPath` (from `savedOutputPath`) / `artifactPaths` (`:282-284`). Its input at
//! `src/runs/foreground/chain-execution.ts:975` is the full `parallelResults: SingleResult[]`,
//! which genuinely carry all four (`execution.ts:847` `result.exitCode = exitCode`, `:963`
//! `result.savedOutputPath = resolvedOutput.savedPath`, `:1114` `artifactPaths`).
//!
//! Before this change `spawn/chain_graph.rs`'s `DynamicGroup` arm built every `CollectChildResult`
//! with `exit_code: Some(i64::from(!sr.success))`, `timed_out: false`, `saved_output_path: None`
//! and `artifact_paths: None`, because the narrow `StepResult` seam carried none of them. The
//! consequences were all observable through `{outputs.<collect.as>}`: a child killed by the run
//! deadline was indistinguishable from an ordinary failure, every failure reported exactly `1`
//! whatever the child's real code, and a later chain step could not locate the files its
//! fanned-out siblings wrote (a `collect.outputSchema` requiring `outputPath` — legal upstream —
//! failed validation outright).
//!
//! Driven through the real `walk_chain` with a scripted `SingleStepExecutor`, the same shape
//! `tests/dynamic_group_acceptance_parity.rs` uses: what is under test is the walker's fold, and a
//! scripted executor is the only way to pin an exact exit code / deadline kill / saved path per
//! child deterministically. The `exec::run_sync` -> `StepResult` half of the same wiring is proven
//! against a REAL child in `tests/chain_step_child_detail_integration.rs`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::Arc;

use cyrup_core::CancelToken;
use cyrup_ext_subagents::error::SubagentError;
use cyrup_ext_subagents::spawn::chain_graph::{
    ChainRunContext, DynamicGroupSpec, OnEmpty, OutputRegistry, RunnerStep, SingleStepExecutor,
    SingleStepSpec, StepResult, walk_chain,
};
use cyrup_ext_subagents::spawn::parallel::GlobalConcurrencyLimit;
use serde_json::{Value, json};

/// A [`SingleStepExecutor`] that answers each child by looking its task text up in a scripted
/// table, so a test can pin one exact outcome per fanned-out item regardless of the order the
/// bounded worker pool happens to run them in.
struct Scripted {
    make: fn(&str) -> StepResult,
}

#[async_trait::async_trait]
impl SingleStepExecutor for Scripted {
    async fn run_single(
        &self,
        _step: &SingleStepSpec,
        resolved_task: &str,
        _ctx: &ChainRunContext,
    ) -> Result<StepResult, SubagentError> {
        Ok((self.make)(resolved_task))
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

fn dynamic_step(collect_schema: Option<Value>) -> DynamicGroupSpec {
    DynamicGroupSpec {
        expand: "outputs.targets".to_string(),
        template: Box::new(template("worker", "Handle {item.id}")),
        collect: "handled".to_string(),
        concurrency: 2,
        fail_fast: false,
        item: None,
        key: None,
        max_items: Some(8),
        on_empty: OnEmpty::Skip,
        collect_schema,
        acceptance: None,
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
        original_task: "Handle everything".to_string(),
        chain_dir: None,
        dynamic_fanout_max_items: None,
    }
}

/// Drive one dynamic step over `source` and return the collect-record ARRAY the step publishes as
/// its own `structured_output` — the SAME `collected_value` that, on the all-children-succeeded
/// path, is registered under `collect.as` for a later `{outputs.handled}` reference (pi
/// `chain-execution.ts:961`: `outputs[collect.as] = { structured: collected }`). Read from the step
/// result rather than the registry so a fan-out with a FAILING child is observable too: pi
/// registers the collect output only when every child succeeded, and this port matches it.
async fn collected(
    make: fn(&str) -> StepResult,
    source: Value,
    collect_schema: Option<Value>,
) -> (Vec<StepResult>, Vec<Value>) {
    let graph = vec![RunnerStep::DynamicGroup(dynamic_step(collect_schema))];
    let executor: Arc<dyn SingleStepExecutor> = Arc::new(Scripted { make });
    let mut registry = OutputRegistry::new();
    registry.register("targets", source);
    let (results, _groups) = walk_chain(&graph, &mut registry, &executor, &run_ctx())
        .await
        .expect("the walk itself must not error");
    let records = results[0]
        .structured_output
        .as_ref()
        .and_then(Value::as_array)
        .expect("a dynamic step publishes the collect-record array as its structured output")
        .clone();
    (results, records)
}

// =================================================================================================
// The child's REAL exit code reaches the record — `dynamic-fanout.ts:278`.
// =================================================================================================

/// pi copies `result.exitCode` through untouched, so a child killed by a signal (`137`) or exiting
/// with a domain-specific code (`2`) is distinguishable in `{outputs.handled}`. Deriving the field
/// from `success` collapses both to `1`.
#[tokio::test]
async fn a_failing_child_reports_its_own_exit_code_not_a_flattened_one() {
    fn make(task: &str) -> StepResult {
        let mut result = StepResult::failure(format!("child failed: {task}"));
        result.exit_code = Some(if task.contains("alpha") { 137 } else { 2 });
        result
    }

    let (_results, records) =
        collected(make, json!([{ "id": "alpha" }, { "id": "beta" }]), None).await;

    assert_eq!(records.len(), 2, "one record per source item, in order");
    assert_eq!(
        records[0]["exitCode"],
        json!(137),
        "the SIGKILLed child's own code must survive into the collect record: {records:#?}"
    );
    assert_eq!(
        records[1]["exitCode"],
        json!(2),
        "the second child's own code must survive too: {records:#?}"
    );
}

/// A clean child still reports `0`, and an executor that runs no real child at all (every scripted
/// or mock executor, which leaves `exit_code` at `None`) still falls back to pi's success/failure
/// shape rather than emitting `null` — the pre-existing behavior other tests depend on.
#[tokio::test]
async fn a_child_with_no_reported_code_falls_back_to_the_success_mapping() {
    fn make(task: &str) -> StepResult {
        if task.contains("alpha") {
            StepResult::success(Some("ok".to_string()), None)
        } else {
            StepResult::failure("nope")
        }
    }

    let (_results, records) =
        collected(make, json!([{ "id": "alpha" }, { "id": "beta" }]), None).await;

    assert_eq!(records[0]["exitCode"], json!(0));
    assert_eq!(records[1]["exitCode"], json!(1));
}

// =================================================================================================
// timedOut / outputPath / artifactPaths — `dynamic-fanout.ts:282-284`.
// =================================================================================================

/// All three conditional spreads at once: a deadline-killed child, and a clean sibling that wrote
/// both a saved-output file and its artifact quadruple. pi emits each key ONLY when the underlying
/// value is truthy, which is why the clean sibling carries no `timedOut` at all.
#[tokio::test]
async fn timed_out_output_path_and_artifact_paths_all_reach_the_record() {
    fn make(task: &str) -> StepResult {
        if task.contains("alpha") {
            let mut result = StepResult::failure("Subagent timed out after 1000ms.");
            result.exit_code = Some(124);
            result.timed_out = true;
            result
        } else {
            let mut result = StepResult::success(Some("wrote it".to_string()), None);
            result.exit_code = Some(0);
            result.saved_output_path = Some("/runs/chain/beta-report.md".to_string());
            result.artifact_paths = Some(json!({
                "inputPath": "/artifacts/run_beta_input.md",
                "outputPath": "/artifacts/run_beta_output.md",
                "jsonlPath": "/artifacts/run_beta.jsonl",
                "metadataPath": "/artifacts/run_beta_meta.json",
            }));
            result
        }
    }

    let (_results, records) =
        collected(make, json!([{ "id": "alpha" }, { "id": "beta" }]), None).await;

    // The deadline kill is visible as its own flag, not folded into an anonymous failure.
    assert_eq!(
        records[0]["timedOut"],
        json!(true),
        "a deadline-killed child must carry `timedOut: true`: {records:#?}"
    );
    assert_eq!(records[0]["exitCode"], json!(124));

    // pi's conditional spread: a child that did NOT time out emits no `timedOut` key at all.
    assert!(
        records[1].get("timedOut").is_none(),
        "`timedOut` is omitted, never `false`, for a child that finished in time: {records:#?}"
    );
    assert_eq!(
        records[1]["outputPath"],
        json!("/runs/chain/beta-report.md"),
        "`outputPath` comes from the child's `savedOutputPath`: {records:#?}"
    );
    assert_eq!(
        records[1]["artifactPaths"]["outputPath"],
        json!("/artifacts/run_beta_output.md"),
        "the artifact quadruple is spread through verbatim: {records:#?}"
    );
    // ...and the timed-out child, which produced neither, carries neither key.
    assert!(records[0].get("outputPath").is_none());
    assert!(records[0].get("artifactPaths").is_none());
}

/// The backlog's own worked example: a `collect.outputSchema` that REQUIRES `outputPath` — legal
/// upstream, because pi's records genuinely carry it — must validate rather than failing the whole
/// dynamic step with `Collected output validation failed: ...`.
#[tokio::test]
async fn a_collect_schema_requiring_output_path_validates() {
    fn make(task: &str) -> StepResult {
        let mut result = StepResult::success(Some("done".to_string()), None);
        result.exit_code = Some(0);
        let id = if task.contains("alpha") { "alpha" } else { "beta" };
        result.saved_output_path = Some(format!("/runs/chain/{id}.md"));
        result
    }

    let schema = json!({
        "type": "array",
        "items": {
            "type": "object",
            "required": ["outputPath"],
            "properties": { "outputPath": { "type": "string" } }
        }
    });

    let (results, records) = collected(
        make,
        json!([{ "id": "alpha" }, { "id": "beta" }]),
        Some(schema),
    )
    .await;

    assert_eq!(results.len(), 1, "one dynamic step, one step result");
    assert!(
        results[0].success,
        "the collect-schema validation must pass: {:?}",
        results[0].error
    );
    assert_eq!(records[0]["outputPath"], json!("/runs/chain/alpha.md"));
    assert_eq!(records[1]["outputPath"], json!("/runs/chain/beta.md"));
}
