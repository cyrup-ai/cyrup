//! The `subagent` tool's `refine` / `refine.show` / `refine.rollback` arm — pi
//! `subagent-executor.ts:6358-6380` @v0.68.0.
//!
//! [`crate::exec::agent_refinements::action::handle_refinement_action`] deliberately names no
//! executor: upstream's `RefinementActionContext.launchProposalChild`
//! (`agents/agent-refinements.ts:100-111`) is an injected closure, and keeping that seam is what
//! makes every write path in the handler testable without a child process. This file is the one
//! place the injection is satisfied with a real run.
//!
//! # Why a real foreground subagent, and not the watchdog's in-process turn
//!
//! Upstream's launch (`subagent-executor.ts:6370-6378`) is `execute(...)` — the same public
//! execution entry point the tool's SINGLE mode uses — with `agent: "reviewer"`,
//! `context: "fresh"`, `async: false`, `artifacts: false`, the schema, and
//! `toolBudget: { hard: 1, block: ["write","edit","bash"] }`. Every one of those has an exact
//! field on [`crate::extension::executor::requests::ForegroundRunRequest`] /
//! [`crate::extension::executor::requests::SingleRunOverrides`] already in tree. The watchdog's
//! nested turn (`watchdog/agent_turn.rs:430-512`) is the wrong seam for three checked reasons:
//!
//! 1. **It cannot carry a structured-output schema.** It builds a bare `Agent::builder` with
//!    `system_prompt`/`thinking_level`/`tools`/`hooks`/`key_resolver`/`tool_execution` and returns
//!    assistant messages. `proposal_schema()` — which is SIX of the refusals guarding this write
//!    path — would simply not be applied, leaving only the validator.
//! 2. **It is the permission arbiter's transport, not a subagent.** Its own doc scopes it to
//!    `permission-arbiter.ts`/`review.ts`; it has no persona, no `reviewer` resolution, no run id
//!    and no tool budget. Upstream's proposal child is a subagent, and the snapshot records the
//!    persona (`agent-refinements.ts:610`).
//! 3. **The tool-budget block list is the read-only enforcement**, and only the subagent path has
//!    one. `{hard: 1, block: ["write","edit","bash"]}` is what makes `proposalTask:517`'s "Do not
//!    read files. Do not use write, edit, or shell tools" more than advice.
//!
//! [CYRUP-DELTA] upstream also passes `preserveActiveSession = true` (`:6378`), which suppresses
//! two parent-state writes: `deps.state.baseCwd = ctx.cwd` (`:5024`) and
//! `deps.state.currentSessionId = resolveCurrentSessionId(...)` (`:6516`). There is nothing to
//! port, and the reason is that the two writes do not exist here rather than that something
//! reproduces the flag: `grep -n "current_session_id" extension/executor/foreground.rs` finds only
//! READS of `self.current_session_id()`, and the executor has no `base_cwd` field at all.

use std::path::Path;
use std::sync::Arc;

use cyrup_core::{CancelToken, TerminateHint, ToolError, ToolResult};

use crate::error::SubagentError;
use crate::exec::agent_refinements::action::{
    RefinementAction, RefinementActionContext, RefinementActionOutcome, handle_refinement_action,
};
use crate::exec::agent_refinements::proposal::{PROPOSAL_AGENT, ProposalChildOutcome};
use crate::extension::SubagentExecutor;
use crate::extension::executor::requests::{ForegroundRunRequest, SingleRunOverrides};
use crate::extension::tool::SubagentTool;
use crate::extension::tool::params::SubagentToolParams;
use crate::fork_context::ContextRequest;

/// pi `subagent-executor.ts:6366-6379` — build the action context and run the verb.
///
/// A free function over the EXECUTOR rather than a method on [`SubagentTool`], because the tool is
/// not the only production surface: `/subagents-refine <agent>` reaches the same verb, and
/// upstream's own refusal sentence (`agents/agent-refinements.ts:548`) names that command. Routing
/// both through one body is the rule `/subagents-guide` and the `guide` action already follow —
/// the two surfaces cannot drift because there is only one of them.
///
/// The child-safe gate is NOT applied here: it belongs to the tool boundary (`:6359` fires inside
/// `route_action`'s guard arm, above this call), and the slash surface is the root interactive
/// parent by construction.
///
/// # Errors
///
/// [`SubagentError::Management`] carrying
/// [`crate::exec::agent_refinements::action::RefinementActionError`]'s own sentence — pi renders
/// every refusal as `result(text, true)`, and this is the crate's error channel for a management
/// verb's own wording.
pub(crate) async fn run_refinement_action(
    executor: &Arc<SubagentExecutor>,
    verb: RefinementAction,
    requested_agent: Option<&str>,
    cwd: &Path,
    cancel: &CancelToken,
) -> Result<RefinementActionOutcome, SubagentError> {
    let cfg = executor.config_snapshot().await;
    // pi `resolveOneAgent:539` re-runs `discoverAgents(cwd, "both")` per call. The scope is the
    // default (both user and project), which is what `discover_agents(cfg, None)` gives.
    let agents = executor
        .discovery_config(cwd, &cfg.roots)
        .and_then(|cfg| crate::discovery::discover_agents(&cfg, None))?;

    // pi `ctx.state` (`:6368`). `include_history: false` because upstream's collector reads only
    // the in-memory `state.asyncJobs`; `fleet_inspector_open: false` because this is not a widget
    // render and `true` would make the fleet-status widget unregister itself
    // (`tui/fleet_state.rs`'s own note on the flag).
    //
    // Built for every verb rather than only for `refine` so the context is one shape; the two
    // read/rollback verbs never look at it. The builder is the SAME
    // `SubagentExecutor::fleet_state` (`extension/executor/status.rs:200`) the RPC bridge, the
    // slash surfaces and the fleet widget already use, used as-is.
    let state = executor.fleet_state(cwd, false, false).await;

    let launch = |task: String, schema: serde_json::Value| {
        let cancel = cancel.clone();
        Box::pin(async move { launch_proposal_child(executor, cwd, task, schema, cancel).await })
            as std::pin::Pin<Box<dyn std::future::Future<Output = ProposalChildOutcome> + Send>>
    };

    handle_refinement_action(
        verb,
        requested_agent,
        RefinementActionContext {
            cwd,
            state: &state,
            agents: &agents,
            now_ms: crate::time::now_epoch_millis(),
            launch_proposal_child: &launch,
        },
    )
    .await
    .map_err(|err| SubagentError::Management(err.to_string()))
}

/// pi's `launchProposalChild` closure (`subagent-executor.ts:6370-6378`), parameter for parameter.
async fn launch_proposal_child(
    executor: &Arc<SubagentExecutor>,
    cwd: &Path,
    task: String,
    schema: serde_json::Value,
    cancel: CancelToken,
) -> ProposalChildOutcome {
    let overrides = SingleRunOverrides {
        // pi `artifacts: false` (`:6375`) — `enabled = artifacts !== false`, so an explicit
        // `Some(false)` is what turns the artifact quadruple off.
        artifacts: Some(false),
        // pi `outputSchema` (`:6376`) — the six schema-side refusals.
        output_schema: Some(schema),
        // pi `toolBudget: { hard: 1, block: ["write","edit","bash"] }` (`:6377`). This is the
        // read-only enforcement: the prompt's "do not use write, edit, or shell tools" is
        // advice, and this is the control.
        tool_budget: Some(crate::discovery::types::ResolvedToolBudget {
            hard: 1,
            soft: None,
            block: crate::discovery::types::ToolBudgetBlock::Names(vec![
                "write".to_string(),
                "edit".to_string(),
                "bash".to_string(),
            ]),
        }),
        ..SingleRunOverrides::default()
    };
    let request = ForegroundRunRequest {
        overrides,
        cwd,
        // pi `agent: "reviewer"` (`:6371`) — the same constant the snapshot records.
        agent_name: PROPOSAL_AGENT,
        task: &task,
        agent_scope: crate::discovery::types::AgentReadScope::default(),
        // pi `context: "fresh"` (`:6373`).
        context: Some(ContextRequest::Fresh),
        model_override: None,
        timeout_ms: None,
        cancel,
        parent_workflow_run_id: None,
        workflow_key: None,
        workflow_steer: None,
    };
    // pi passes `onUpdate: undefined` (`:6378`) — the proposal child streams no progress — so
    // the sink is a no-op closure. `run_foreground_streaming` is the entry point that takes a
    // `ForegroundRunRequest` (and with it the host's real cancellation token); the flat
    // `run_foreground` overload mints a fresh, never-cancelled token, which would drop
    // upstream's `ctx.signal` threading on the floor.
    let sink: cyrup_core::ToolUpdateSink = Box::new(|_| {});
    match executor.run_foreground_streaming(request, sink).await {
        Ok((result, _run_id)) => ProposalChildOutcome {
            // pi `child.isError` (`:596`). A child that exited non-zero or reported an error
            // is a failed proposal, and `handle_refinement_action` writes nothing.
            is_error: result.exit_code != 0 || result.error.is_some(),
            structured_output: result.structured_output.clone(),
            final_output: result
                .final_output
                .clone()
                .filter(|out| !out.trim().is_empty()),
        },
        // A launch that could not happen at all is upstream's `child.isError` too: `:596`
        // reports one sentence for every child-side failure. That sentence is fixed and carries
        // no reason, so the reason is LOGGED rather than dropped — otherwise a misconfigured
        // `reviewer` persona is indistinguishable, from the outside, from a child that ran and
        // produced nothing usable.
        Err(err) => {
            tracing::warn!(error = %err, "refinement proposal child could not be launched");
            ProposalChildOutcome {
                is_error: true,
                structured_output: None,
                final_output: None,
            }
        }
    }
}

impl SubagentTool {
    /// pi `subagent-executor.ts:6366-6379` at the TOOL boundary: the child-safe gate has already
    /// fired in `route_action`'s guard arm (upstream's own order — `:6359` precedes this call), so
    /// this only adapts [`run_refinement_action`]'s outcome to a [`ToolResult`].
    pub(crate) async fn route_refinement_action(
        &self,
        verb: RefinementAction,
        p: &SubagentToolParams,
        cwd: &Path,
        cancel: &CancelToken,
    ) -> Result<ToolResult, ToolError> {
        let outcome = run_refinement_action(&self.executor, verb, p.agent.as_deref(), cwd, cancel)
            .await
            // pi renders every refusal as `result(text, true)`; cyrup's error channel is
            // `Err(ToolError)`, which the runtime renders the same way — the shape the `lane.*`
            // arm already uses for `CleanupPlanError`.
            .map_err(|err| ToolError::new(err.to_string()))?;
        if outcome.is_error {
            return Err(ToolError::new(outcome.text));
        }
        Ok(ToolResult {
            content: vec![cyrup_core::Content::text(outcome.text)],
            // pi `result(...)` (`agents/agent-refinements.ts:113-119`) sets
            // `details: { mode: "management", results: [] }` on EVERY outcome — the same shape the
            // lane arm writes.
            details: Some(serde_json::json!({ "mode": "management", "results": [] })),
            terminate: TerminateHint::Unspecified,
            ..Default::default()
        })
    }
}
