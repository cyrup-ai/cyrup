//! SUBA-100 — the pane-native external-cli branch (`subagent-runner.ts:900-944` @v0.68.0): an
//! owned claude/codex/cursor profile whose run carries a resolved Herdr machine is launched in a
//! fresh Herdr-owned pane ON that machine rather than as a local foreign process, prompted through
//! Herdr's own `agent.prompt`, and settled from bounded terminal evidence.
//!
//! Upstream settles EVERY placed external run at `exitCode: 1` with `execution: { status:
//! "partial" }` (`:928`): the only evidence is what the pane showed, which is never a verified
//! completion, and the output is tagged `[best-effort/unverified]` accordingly. The same
//! `execution` block rides [`SingleResult::execution`], and it is what lets a run whose only
//! non-success is such a result settle `partial` rather than `failed`
//! ([`crate::exec::run_result::partial_evidence`]). A stopped run settles `execution: { status:
//! "stopped" }` (`:917`).

use crate::exec::run_result::SingleResult;
use crate::exec::{AgentConfig, RunOptions};
use crate::placement::HerdrMachineReference;
use crate::runner::contract::AdapterId;
use crate::runner::status::ExternalCliRunnerStatus;

/// Everything [`run_placed_external_cli`] needs beyond the run's own options.
pub(super) struct PlacedExternalCli<'a> {
    /// The owned adapter (a generic command was refused before this point).
    pub adapter: AdapterId,
    /// The resolved machine.
    pub machine: &'a HerdrMachineReference,
    /// `buildExternalCliPrompt(systemPrompt, task)` over the post-acceptance task.
    pub prompt: &'a str,
    /// The runner descriptor, stamped with the machine before it is published.
    pub status: ExternalCliRunnerStatus,
    /// The output file's pre-run snapshot, for the saved-output handoff.
    pub output_snapshot: Option<crate::exec::output::OutputFileSnapshot>,
}

/// Run one placed external profile and settle it as upstream does.
pub(super) async fn run_placed_external_cli(
    agent: &AgentConfig,
    task: &str,
    opts: &RunOptions,
    placed: PlacedExternalCli<'_>,
) -> SingleResult {
    let PlacedExternalCli {
        adapter,
        machine,
        prompt,
        mut status,
        output_snapshot,
    } = placed;
    status.machine = Some(machine.clone());
    // pi `placedRunId = `${ctx.id}-${ctx.flatIndex}`` — the run id plus the child's flat index.
    let run_id = opts
        .run_id
        .as_ref()
        .map(|run_id| format!("{}-{}", run_id.as_str(), opts.child_index.unwrap_or(0)));
    let deadline = opts
        .deadline_at
        .map(tokio::time::Instant::from_std)
        .or_else(|| {
            opts.timeout_ms
                .map(|ms| tokio::time::Instant::now() + std::time::Duration::from_millis(ms))
        });
    let agent_dir = machine.agent_dir();
    let outcome = crate::placement::external::run_placed_external(
        crate::placement::external::PlacedExternalInput {
            adapter,
            machine,
            run_id: run_id.as_deref(),
            prompt,
            deadline,
            cancel: &opts.cancel,
            transport: machine.ssh_transport(),
            agent_dir: &agent_dir,
        },
    )
    .await;

    if let Some(error) = outcome.error {
        // Setup could not complete — upstream's `prepareInternalHerdrExternalAdapter` throw, which
        // the runner surfaces as the step's error, verbatim.
        let mut result = crate::exec::pre_spawn_failure(agent, task, error);
        result.runner = Some(status);
        return result;
    }

    let output = outcome.output;
    // pi `resolveSingleOutput(step.outputPath, output, …)` + `finalizeSingleOutput({ …, exitCode:
    // 1, preserveSavedOutput: true })`: a placed run's evidence IS saved and referenced despite the
    // partial exit code. cyrup's two helpers gate on a clean exit, so they are handed the
    // "clean" code here — which is exactly what upstream's `preserveSavedOutput` switch means.
    let mut error = None;
    let (final_output, full_output_for_reference, saved_output_path) =
        crate::exec::resolve_saved_output(
            opts,
            0,
            (!output.is_empty()).then(|| output.clone()),
            output_snapshot,
            &mut error,
        );
    let (final_output, output_truncated) = crate::exec::finalize_delivered_output(
        final_output,
        full_output_for_reference,
        saved_output_path.as_deref(),
        false,
        0,
        agent.max_output,
        opts.output_mode,
    );

    let mut result = crate::exec::pre_spawn_failure(agent, task, String::new());
    result.error = error;
    result.exit_code = 1;
    result.output_state = crate::exec::output_state::derive_output_state(
        (!output.trim().is_empty()).then_some(output.as_str()),
        None,
        None,
    );
    result.final_output = final_output;
    result.output_truncated = output_truncated;
    result.saved_output_path = saved_output_path.map(|path| path.display().to_string());
    result.timed_out = outcome.timed_out;
    result.stopped = outcome.stopped;
    result.execution = Some(if outcome.stopped {
        crate::exec::run_result::ExecutionOutcome::stopped()
    } else {
        crate::exec::run_result::ExecutionOutcome::partial()
    });
    result.runner = Some(status);
    result
}
