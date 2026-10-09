//! SUBA-074 stage 2 — executing an agent whose `runner:` declares an external CLI.
//!
//! Upstream's execution branch is `subagent-runner.ts:1491-1566` (@v0.64.0): resolve the adapter's
//! launch, publish an [`ExternalCliRunnerStatus`], run the foreign process, resolve the output
//! handoff, and persist a receipt. Crucially it NEVER enters the model-fallback ladder — a run with
//! an external runner resolves no model at all (`api/preflight.ts:317-341`: `primaryModel =
//! undefined`, `modelCandidates = []`, no thinking ceiling), which is why
//! [`crate::runner::dispatch`] decides the dispatch before [`crate::exec::run_sync`] builds its
//! candidate list.
//!
//! **What ships in this batch.** The generic no-adapter path (upstream's in-baseline `v0.43.0`
//! runner) and the `claude-code`/`claude-code-writer` adapter. `codex-exec`, `cursor-agent` and the
//! whole `external-job` protocol stay REFUSED, loudly, by [`crate::runner::dispatch`] — see that
//! module for each deferral's reason.

pub mod adapters;
pub mod env;
pub mod framing;
mod placed;
pub mod preflight;
pub mod prompt;
pub mod run;

use std::path::PathBuf;

use cyrup_core::Usage;

use crate::exec::run_result::SingleResult;
use crate::exec::{AgentConfig, RunOptions};
use crate::runner::contract::AdapterId;
use crate::runner::status::{ExternalCliRunnerStatus, resolve_external_cli_runner_status};
use adapters::{AdapterParser, claude_code};
use env::ExternalEnv;
use framing::StreamLimits;
use preflight::PreflightSpec;
use prompt::{PromptDelivery, build_external_cli_prompt};

/// Everything a launch resolver needs that comes from the RUN rather than from the agent file.
///
/// Passed by [`crate::runner::dispatch::resolve_runner_dispatch`], which is pure — nothing here is
/// read from the filesystem or the clock.
///
/// It carries ONLY what a shipped resolver reads. Upstream's `resolveCodexExecLaunch` /
/// `resolveCursorAgentLaunch` also take `cwd`, `asyncDir` and `stepIndex`
/// (`subagent-runner.ts:1493-1498` @v0.64.0), and those three were carried here from the start of
/// stage 2 — but neither of those adapters ships (both are REFUSED by
/// [`crate::runner::dispatch`]), the two resolvers that DO ship never read them, and
/// [`run_external_cli`] derives the same values from [`RunOptions`] for itself. Populating a field
/// nothing reads invites exactly the drift where the context's copy and the runner's copy stop
/// agreeing, so they are not here: the adapter that needs them reintroduces them with its
/// resolver.
#[derive(Debug, Clone, Default)]
pub struct ExternalCliLaunchContext {
    /// Upstream's `commandPrefixArgs` test seam (`claude-code-adapter.ts:84-85`): argv placed in
    /// FRONT of the adapter's own, so a test can point `command` at an interpreter and still
    /// exercise the real flags against a fake process. Empty in production.
    pub command_prefix_args: Vec<String>,
    /// SUBA-167 — upstream's `overrideArgs` (`claude-code-adapter.ts:233-234,263` @v0.76.1): the
    /// `--model`/`--effort` tokens [`resolve_claude_code_launch_override`] resolved for this run,
    /// appended AFTER the adapter's fixed argv. Read only by the Claude Code resolver;
    /// [`resolve_generic_launch`] ignores it.
    pub claude_code_override_args: Vec<String>,
}

/// A resolved external-CLI launch — the proof that this build can actually execute the declared
/// runner.
///
/// There is deliberately no public constructor: the only ways to obtain one are
/// [`resolve_generic_launch`] and the per-adapter resolvers, so
/// [`crate::runner::dispatch::RunnerDispatch::ExternalCli`] cannot be produced for a runner nothing
/// knows how to launch.
#[derive(Debug)]
pub struct ExternalCliLaunch {
    /// The published runner descriptor for this launch.
    status: ExternalCliRunnerStatus,
    /// The binary to execute.
    command: String,
    /// The complete argv.
    args: Vec<String>,
    /// The child's environment.
    environment: ExternalEnv,
    /// The binary probe, when the adapter demands one.
    preflight: Option<PreflightSpec>,
    /// The stream parser, when the adapter has one.
    parser: Option<AdapterParser>,
    /// The prompt's single delivery channel.
    delivery: PromptDelivery,
    /// An adapter-owned final-output artifact.
    final_output_path: Option<PathBuf>,
}

impl ExternalCliLaunch {
    /// The runner descriptor this launch publishes — read by the dispatch tests and by the caller
    /// that persists the receipt.
    #[must_use]
    pub fn status(&self) -> &ExternalCliRunnerStatus {
        &self.status
    }

    /// The argv this launch will spawn, for tests that pin an adapter's flags end to end.
    #[must_use]
    pub fn args(&self) -> &[String] {
        &self.args
    }

    /// The child's environment policy.
    #[must_use]
    pub fn environment(&self) -> &ExternalEnv {
        &self.environment
    }
}

/// The generic `external-cli` path: an author-declared `command`/`args`, the parent environment,
/// no preflight and no parser.
///
/// This is upstream's IN-BASELINE runner (`v0.43.0:src/runs/shared/external-cli-runner.ts`), which
/// cyrup never ported either — so shipping it converts SUBA-074 from "window lag plus baseline lag"
/// into window lag alone. It needs no vendor CLI, which is why it is the path the crate's own
/// end-to-end test drives.
#[must_use]
pub fn resolve_generic_launch(
    cli: &crate::runner::ExternalCliRunner,
    _ctx: &ExternalCliLaunchContext,
) -> ExternalCliLaunch {
    ExternalCliLaunch {
        status: resolve_external_cli_runner_status(None, &cli.command, &cli.args),
        command: cli.command.clone(),
        args: cli.args.clone(),
        // Upstream's own choice at `external-cli-runner.ts:89`: with no adapter allowlist, the
        // child inherits the parent environment.
        environment: ExternalEnv::Inherited,
        preflight: None,
        parser: None,
        delivery: PromptDelivery::Stdin,
        final_output_path: None,
    }
}

/// `resolveClaudeCodeLaunch` (`claude-code-adapter.ts:80-129`).
///
/// # Errors
///
/// Only [`env::ExternalEnv::allowlisted`]'s refusals, which are unreachable for the code-owned
/// allowlist but are surfaced rather than unwrapped because this crate denies `unwrap` outside
/// tests.
pub fn resolve_claude_code_launch(
    adapter: AdapterId,
    cli: &crate::runner::ExternalCliRunner,
    ctx: &ExternalCliLaunchContext,
) -> Result<ExternalCliLaunch, String> {
    let args = claude_code::launch_args(
        adapter,
        &ctx.command_prefix_args,
        &ctx.claude_code_override_args,
    );
    // The probes are built from the prefix ALONE, so the per-launch override tokens never reach
    // `--version`/`--help` (`claude-code-adapter.ts:261-262,271-272` @v0.76.1).
    let mut version_args = ctx.command_prefix_args.clone();
    version_args.push("--version".to_string());
    let mut help_args = ctx.command_prefix_args.clone();
    help_args.push("--help".to_string());
    Ok(ExternalCliLaunch {
        // The status carries the ADAPTER's argv, not the author's — upstream overrides
        // `args` with `adapterLaunch.args` at `subagent-runner.ts:1500`.
        status: resolve_external_cli_runner_status(Some(adapter), &cli.command, &args),
        command: cli.command.clone(),
        args,
        environment: ExternalEnv::allowlisted(&claude_code::CLAUDE_CODE_ENV_ALLOWLIST, &[])?,
        preflight: Some(PreflightSpec {
            id: adapter.wire().to_string(),
            version_args,
            help_args,
            probe_timeout_ms: None,
            required_help: claude_code::required_help(adapter),
            version_validator: Some(claude_code::validate_version),
        }),
        parser: Some(AdapterParser::ClaudeCode(
            claude_code::ClaudeCodeParser::new(),
        )),
        delivery: PromptDelivery::Stdin,
        final_output_path: None,
    })
}

/// SUBA-167 — the per-launch `--model`/`--effort` tokens for a Claude Code agent, or an empty list
/// for every other runner.
///
/// Upstream runs the same three calls at both of its background launch sites
/// (`async-execution.ts:1100-1116` chain/parallel, `:1917-1932` single @v0.76.1):
/// `resolveClaudeCodeOverride`, then `assertClaudeCodeModelScope`, then
/// `assertClaudeCodeOverrideIsLocal`, each throw refusing the launch before it starts.
/// [`crate::exec::run_sync`] is the one chokepoint cyrup's foreground single, chain/parallel and
/// hop-2 runner launches all reach, so it calls this once, ahead of the runner dispatch.
///
/// - The model is [`RunOptions::launch_model`] — the caller's or step's model AS TYPED, never the
///   post-inheritance [`RunOptions::model_override`], which may be the parent session's model.
/// - The thinking level is `agent.thinking`, which already holds the caller's explicit per-call
///   `thinking` over the persona's own (upstream's `resolveClaudeCodeThinking(thinkingOverride,
///   a.thinking)`, `:886-889`). Both launch sites leave the parent-session rung OFF for a Claude
///   Code agent (upstream never inherits it for an external runner: `effectiveThinking =
///   externalRunner ? undefined : …`, `:1117`).
/// - The ceiling is folded exactly as `run_sync`'s Step 2b folds it.
///
/// # Errors
///
/// Upstream's verbatim refusal from whichever of the three checks fails, or a malformed inherited
/// ceiling.
pub fn resolve_claude_code_launch_override(
    agent: &AgentConfig,
    opts: &RunOptions,
) -> Result<Vec<String>, String> {
    if !agent
        .runner
        .as_ref()
        .is_some_and(crate::runner::AgentRunnerConfig::is_claude_code)
    {
        return Ok(Vec::new());
    }
    let thinking_ceiling =
        crate::exec::thinking_ceiling::inherited_thinking_ceiling().and_then(|inherited| {
            crate::exec::thinking_ceiling::intersect_thinking_ceilings(&[
                opts.thinking_ceiling.as_deref(),
                inherited.as_deref(),
            ])
        })?;
    let run_id = opts.run_id.as_ref().map(crate::background::RunId::as_str);
    let claude_code_override =
        claude_code::resolve_claude_code_override(claude_code::ClaudeCodeOverrideInput {
            model: opts.launch_model.as_ref().map(cyrup_core::ModelId::as_str),
            agent_model: agent.model.as_ref().map(cyrup_core::ModelId::as_str),
            agent_model_is_settings_default: agent.model_is_settings_default,
            thinking: agent.thinking.as_deref(),
            thinking_ceiling: thinking_ceiling.as_deref(),
            agent_name: Some(agent.name.as_str()),
            run_id,
        })?;
    let scopes = crate::exec::model_scope::resolve_model_scopes_for_agent(
        opts.model_scope.as_ref(),
        &agent.name,
        opts.parent_model.as_ref().map(cyrup_core::ModelId::as_str),
    );
    claude_code::assert_claude_code_model_scope(
        &scopes,
        claude_code_override
            .as_ref()
            .and_then(|resolved| resolved.model.as_deref()),
        &agent.name,
        run_id,
    )?;
    // [CYRUP-DELTA] upstream names the REQUESTED selector string; a resolved
    // [`crate::placement::HerdrMachineReference`] is all `RunOptions` carries, so the refusal
    // names its display name — the same choice the placement refusals in `run_external_cli` make.
    claude_code::assert_claude_code_override_is_local(
        &agent.name,
        opts.machine
            .as_ref()
            .map(crate::placement::HerdrMachineReference::display_name),
        claude_code_override.as_ref(),
    )?;
    Ok(claude_code_override.map_or_else(Vec::new, |resolved| resolved.args))
}

/// Execute an external-CLI runner and lower its outcome into a [`SingleResult`].
///
/// The result carries `model: None` and empty `attempted_models`/`model_attempts` — upstream
/// resolves NO model for an external runner at all (`api/preflight.ts:322-343`), and a "helpful"
/// ladder entry here would misreport which model produced the work.
///
/// `contract` is the run's effective acceptance contract, injected into the task the FOREIGN
/// process sees — see the call site below.
pub async fn run_external_cli(
    agent: &AgentConfig,
    task: &str,
    opts: &RunOptions,
    contract: &crate::exec::acceptance::AcceptanceContract,
    launch: ExternalCliLaunch,
) -> SingleResult {
    let ExternalCliLaunch {
        status,
        command,
        args,
        environment,
        preflight: preflight_spec,
        parser,
        delivery,
        final_output_path,
    } = launch;

    let started_at = crate::time::now_epoch_millis();
    let output_snapshot = crate::exec::output::snapshot_output_file(opts.output_path.as_deref());
    // SUBA-141 — the run's own directory when it has one (pi `asyncDir`, `subagent-runner.ts:924`
    // @v0.71.0), so a live reader can tail the logs inside the run it is reading.
    let scratch_dir = opts
        .external_log_dir
        .clone()
        .unwrap_or_else(|| crate::background::attempt_scratch_dir(&opts.cwd));
    let step_index = opts.child_index.unwrap_or(0);
    // Every pre-spawn failure below still publishes a receipt naming these two, exactly as
    // upstream's pre-spawn `catch` does (`external-cli-runner.ts:212-213`) — an empty
    // `externalProcess` would tell a reader the run never reached the runner at all.
    let (stdout_path, stderr_path) = run::external_log_paths(&scratch_dir, step_index);
    let pre_spawn = |error: String| {
        external_failure(
            agent,
            task,
            &status,
            Some(run::pre_spawn_receipt(
                started_at,
                &stdout_path,
                &stderr_path,
                final_output_path.as_deref(),
            )),
            error,
        )
    };

    // SUBA-096 — pi refuses fast mode for a foreign runner before anything launches
    // (`subagent-executor.ts:3353-3355`, `async-execution.ts:1012,1756` @v0.68.0): the priority
    // tier is a request-body field on pi's OWN provider calls, which a foreign CLI never makes.
    // `opts.fast` is already the effective `step > call > agent` value, so this one site covers
    // the foreground, async and chain-step launches alike. `[CYRUP-DELTA]` (text only): upstream's
    // async paths phrase it inside a list (`does not support: …, fast mode.`); every path here
    // uses the foreground sentence, since cyrup has no multi-item external-runner refusal list.
    if opts.fast {
        return pre_spawn(format!(
            "Agent '{}' uses runner.type='external-cli' and does not support fast mode.",
            agent.name
        ));
    }
    // `buildExternalCliPrompt(step.systemPrompt ?? "", task)` (`subagent-runner.ts:1506`) — over
    // the POST-acceptance task. Upstream appends `formatAcceptancePrompt(step.effectiveAcceptance,
    // …)` to `task` at `:1462-1465`, ABOVE the `if (step.runner?.type === "external-cli")` branch
    // at `:1491`, so the foreign process is told the contract it will be judged against just as a
    // native child is. Only the PROMPT carries it: `SingleResult::task` below stays the raw task,
    // matching the native path, where the injection likewise happens inside prompt construction
    // (`exec/spawn_plan.rs`) and never rewrites the recorded task.
    let prompt_text = build_external_cli_prompt(
        &agent.system_prompt_body,
        &crate::exec::acceptance::inject_acceptance_contract(task, contract),
    );
    // SUBA-100 — a resolved machine sends an OWNED profile to a Herdr pane on that machine
    // (`subagent-runner.ts:900-944` @v0.68.0). Its prompt goes through Herdr's `agent.prompt`, so
    // the local delivery, environment and preflight below are not used. A generic command never
    // reaches here placed — the launch refused it — and the backstop repeats that refusal.
    if let Some(machine) = opts.machine.as_ref() {
        let adapter = match agent.runner.as_ref() {
            Some(crate::runner::AgentRunnerConfig::ExternalCli(cli)) => cli.adapter,
            _ => None,
        };
        let Some(adapter) = adapter else {
            return pre_spawn(
                crate::placement::format_herdr_machine_runner_unsupported(
                    Some(machine.display_name()),
                    &agent.name,
                    agent.runner.as_ref(),
                    false,
                )
                .unwrap_or_else(|| {
                    format!(
                        "Agent '{}' requested a machine, but its runner has no pane-native profile.",
                        agent.name
                    )
                }),
            );
        };
        return placed::run_placed_external_cli(
            agent,
            task,
            opts,
            placed::PlacedExternalCli {
                adapter,
                machine,
                prompt: &prompt_text,
                status,
                output_snapshot,
            },
        )
        .await;
    }
    let prepared = match delivery.prepare(&prompt_text) {
        Ok(prepared) => prepared,
        Err(error) => {
            return pre_spawn(error.to_string());
        }
    };

    let env = environment.materialise(&env::process_env_lookup);

    // `preflightExternalCli` (`external-cli-runner.ts:210`) runs inside upstream's pre-spawn `try`,
    // so a failure settles the run at exit 1 with the probe's own message and NEVER spawns.
    let mut program = PathBuf::from(&command);
    if let Some(spec) = preflight_spec.as_ref() {
        match preflight::preflight_external_cli(&command, spec, env.as_ref(), &opts.cwd).await {
            Ok(result) => program = result.binary_path,
            Err(error) => {
                preflight::invalidate_external_cli_preflight(
                    &command,
                    spec,
                    preflight::classify_invalidation(&error),
                );
                return pre_spawn(error);
            }
        }
    }

    let deadline = opts
        .deadline_at
        .map(tokio::time::Instant::from_std)
        .or_else(|| {
            opts.timeout_ms
                .map(|ms| tokio::time::Instant::now() + std::time::Duration::from_millis(ms))
        });
    // SUBA-141 — pi `onProcess: ctx.onExternalProcess` (`subagent-runner.ts:936` @v0.71.0): each
    // process report goes to the live sink with this launch's runner descriptor, typed.
    let report_process = |process: &crate::runner::status::ExternalProcessStatus| {
        if let Some(sink) = opts.live_events.as_ref() {
            sink.emit_external_process(crate::exec::ExternalProcessUpdate {
                runner: status.clone(),
                process: process.clone(),
            });
        }
    };
    let outcome = run::run_external_cli_process(
        run::ExternalCliProcessPlan {
            program,
            args,
            env,
            parser,
            limits: StreamLimits::default(),
            final_output_path,
        },
        &prepared,
        &run::ExternalCliRunInput {
            cwd: opts.cwd.clone(),
            log_dir: scratch_dir,
            step_index,
            deadline,
            stop: &opts.cancel,
            timeout_message: opts.timeout_ms.map_or_else(
                || "Subagent timed out.".to_string(),
                crate::exec::format_timeout_message,
            ),
            stop_message: "Subagent stopped by user.".to_string(),
            on_process: Some(run::ProcessHook(&report_process)),
        },
    )
    .await;

    // `if (error && input.preflight && !parserError) invalidateExternalCliPreflight(...)` (`:406`).
    if let (Some(error), Some(spec)) = (outcome.error.as_ref(), preflight_spec.as_ref()) {
        preflight::invalidate_external_cli_preflight(
            &command,
            spec,
            preflight::classify_invalidation(error),
        );
    }

    let mut error = outcome.error.clone();
    let final_output = (!outcome.output.is_empty()).then(|| outcome.output.clone());
    // R-SA-031 output handoff, exactly as the native path resolves it (`subagent-runner.ts:1524`
    // gates it on `external.exitCode === 0` for the same reason).
    let (final_output, full_output_for_reference, saved_output_path) =
        crate::exec::resolve_saved_output(
            opts,
            outcome.exit_code,
            final_output,
            output_snapshot,
            &mut error,
        );
    let (final_output, output_truncated) = crate::exec::finalize_delivered_output(
        final_output,
        full_output_for_reference,
        saved_output_path.as_deref(),
        false,
        outcome.exit_code,
        agent.max_output,
        opts.output_mode,
    );

    SingleResult {
        execution: None,
        native_machine: None,
        runtime_acknowledged_extensions: None,
        skills_warning: None,
        watchdog: None,
        agent: agent.name.clone(),
        task: task.to_string(),
        exit_code: outcome.exit_code,
        usage: Usage::default(),
        turns: 0,
        // Upstream resolves no model for an external runner at all.
        model: None,
        attempted_models: Vec::new(),
        model_attempts: Vec::new(),
        // pi's external branch derives `outputState` with the simple two-branch rule
        // (`subagent-runner.ts:927`: `external.output.trim() ? "present" : "absent"`) — the shared
        // derivation with its last two arguments `None`, from the RAW foreign output rather than
        // the finalized delivery.
        output_state: crate::exec::output_state::derive_output_state(
            (!outcome.output.is_empty()).then_some(outcome.output.as_str()),
            None,
            None,
        ),
        session_file: None,
        structured_output_path: None,
        artifact_paths: None,
        // A foreign process has no parsed NDJSON child-event stream to feed the live transcript
        // writer from (it attaches in `exec::drive_attempt`, the native child's drive loop), so an
        // external-cli run never has one.
        transcript_path: None,
        transcript_error: None,
        // A foreign-CLI child has no cyrup run id of its own.
        child_run_id: None,
        final_output,
        structured_output: None,
        acceptance: None,
        detached: false,
        detached_reason: None,
        interrupted: false,
        timed_out: outcome.timed_out,
        // Upstream's external-CLI branch never sets `timeoutRecovery` either (`subagent-runner.ts:1563`
        // builds the external result without it): the mutation snapshot/collect pair is scoped to
        // the native child path, and the foreign process ran under its own sandbox contract.
        timeout_recovery: None,
        // Upstream's external-CLI branch never sets `contextOverflow` (`subagent-runner.ts:1563`
        // builds the external result without it): the flag is a model-fallback-ladder
        // classification, and an external profile never enters the ladder.
        context_overflow: false,
        stopped: outcome.stopped,
        process_signal: outcome.process_signal.clone(),
        turn_budget: None,
        turn_budget_exceeded: false,
        wrap_up_requested: false,
        tool_budget_blocked: false,
        session_name: None,
        usage_budget: None,
        error,
        saved_output_path: saved_output_path.map(|path| path.display().to_string()),
        tool_calls: Vec::new(),
        // An external-cli agent is forbidden to declare `tools:` and never crosses cyrup's own
        // `--tools` seam, so it has no cyrup tool surface. Default (unpinned) is exact: it is also
        // what makes `ResolvedToolSurface::grants` answer `Unknown` for every name on this arm, so
        // the pre-spawn claim gate is a no-op for external runners by construction.
        tool_surface: crate::exec::tool_surface::ResolvedToolSurface::default(),
        output_truncated,
        control_events: Vec::new(),
        progress: None,
        runner: Some(status),
        external_process: Some(outcome.external_process),
    }
}

/// A run that failed before (or instead of) the foreign process producing anything: still carries
/// the runner descriptor, so a caller can see WHICH profile was refused and under what sandbox.
fn external_failure(
    agent: &AgentConfig,
    task: &str,
    status: &ExternalCliRunnerStatus,
    external_process: Option<crate::runner::status::ExternalProcessStatus>,
    error: String,
) -> SingleResult {
    let mut result = crate::exec::pre_spawn_failure(agent, task, error);
    result.runner = Some(status.clone());
    result.external_process = external_process;
    result
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use crate::runner::ExternalCliRunner;

    fn cli(adapter: Option<AdapterId>, command: &str, args: &[&str]) -> ExternalCliRunner {
        ExternalCliRunner {
            adapter,
            command: command.to_string(),
            args: args.iter().map(|a| (*a).to_string()).collect(),
            prompt_delivery_stdin: false,
            capabilities: None,
        }
    }

    /// The generic path is the author's own command line, run with the parent environment and no
    /// probe — upstream's in-baseline behaviour (`v0.43.0:external-cli-runner.ts`).
    #[test]
    fn the_generic_launch_is_the_authors_command_with_an_inherited_environment() {
        let launch = resolve_generic_launch(
            &cli(None, "my-cli", &["--flag"]),
            &ExternalCliLaunchContext::default(),
        );
        assert_eq!(launch.command, "my-cli");
        assert_eq!(launch.args(), ["--flag".to_string()]);
        assert_eq!(launch.environment(), &ExternalEnv::Inherited);
        assert!(launch.preflight.is_none());
        assert!(launch.parser.is_none());
        assert_eq!(launch.delivery, PromptDelivery::Stdin);
        assert_eq!(launch.status().prompt_delivery, "stdin");
        assert!(launch.status().safety.is_none());
    }

    /// The claude-code launch OVERRIDES the author's argv with the adapter's own (the parser
    /// refuses an author who declares `args` alongside an adapter at all), seals the environment to
    /// the 32-key allowlist, and demands a probe.
    #[test]
    fn the_claude_code_launch_owns_its_argv_seals_its_environment_and_demands_a_probe() {
        let launch = resolve_claude_code_launch(
            AdapterId::ClaudeCode,
            &cli(Some(AdapterId::ClaudeCode), "claude", &[]),
            &ExternalCliLaunchContext::default(),
        )
        .unwrap();
        assert!(launch.args().contains(&"--permission-mode".to_string()));
        assert!(launch.args().contains(&"plan".to_string()));
        assert!(launch.args().contains(&"--strict-mcp-config".to_string()));
        assert!(launch.args().contains(&r#"{"mcpServers":{}}"#.to_string()));
        assert_eq!(
            launch.status().args,
            launch.args(),
            "the published status must report the argv that will actually be spawned"
        );
        assert_eq!(
            launch.status().safety.as_ref().unwrap()["permissionMode"],
            "plan"
        );

        // The environment is the allowlist projection, and it carries none of this crate's own
        // subagent configuration.
        let materialised = launch
            .environment()
            .materialise(&|key| match key {
                "PATH" => Some("/usr/bin".to_string()),
                "CYRUP_SUBAGENT_PERMISSION_POLICY" => Some("{}".to_string()),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            materialised.get("PATH").map(String::as_str),
            Some("/usr/bin")
        );
        assert!(!materialised.contains_key("CYRUP_SUBAGENT_PERMISSION_POLICY"));

        let spec = launch.preflight.as_ref().unwrap();
        assert_eq!(spec.id, "claude-code");
        assert_eq!(spec.version_args, vec!["--version".to_string()]);
        assert_eq!(spec.required_help.len(), 14);

        // The writer twin differs where — and only where — its safety block says it does.
        let writer = resolve_claude_code_launch(
            AdapterId::ClaudeCodeWriter,
            &cli(Some(AdapterId::ClaudeCodeWriter), "claude", &[]),
            &ExternalCliLaunchContext::default(),
        )
        .unwrap();
        assert!(writer.args().contains(&"acceptEdits".to_string()));
        assert_eq!(
            writer.status().safety.as_ref().unwrap()["tools"],
            claude_code::CLAUDE_CODE_WRITER_TOOLS
        );
    }

    /// The test seam threads a command prefix into BOTH the argv and the probe argv
    /// (`claude-code-adapter.ts:96-98`, `:118-119`), which is what makes an end-to-end adapter test
    /// hermetic on a machine with no vendor CLI installed.
    #[test]
    fn the_command_prefix_reaches_the_argv_and_both_probes() {
        let ctx = ExternalCliLaunchContext {
            command_prefix_args: vec!["--fake".to_string()],
            ..ExternalCliLaunchContext::default()
        };
        let launch = resolve_claude_code_launch(
            AdapterId::ClaudeCode,
            &cli(Some(AdapterId::ClaudeCode), "claude", &[]),
            &ctx,
        )
        .unwrap();
        assert_eq!(launch.args()[0], "--fake");
        let spec = launch.preflight.as_ref().unwrap();
        assert_eq!(
            spec.version_args,
            vec!["--fake".to_string(), "--version".to_string()]
        );
        assert_eq!(
            spec.help_args,
            vec!["--fake".to_string(), "--help".to_string()]
        );
    }

    /// SUBA-167 — upstream `appends an override after the fixed argv and keeps it out of
    /// preflight` (`test/unit/claude-code-adapter.test.ts:293-303` @v0.76.1): both adapters END
    /// with the override tokens, the probes stay prefix-only, and the published runner status
    /// carries the final argv including the tokens (`subagent-runner.ts:916,920`).
    #[test]
    fn the_override_is_appended_after_the_fixed_argv_and_kept_out_of_preflight() {
        let override_args =
            claude_code::resolve_claude_code_override(claude_code::ClaudeCodeOverrideInput {
                model: Some("claude-opus-5.5:medium"),
                ..claude_code::ClaudeCodeOverrideInput::default()
            })
            .unwrap()
            .unwrap()
            .args;
        let ctx = ExternalCliLaunchContext {
            claude_code_override_args: override_args,
            ..ExternalCliLaunchContext::default()
        };
        let tail = ["--model", "claude-opus-5.5", "--effort", "medium"];
        for adapter in [AdapterId::ClaudeCode, AdapterId::ClaudeCodeWriter] {
            let launch =
                resolve_claude_code_launch(adapter, &cli(Some(adapter), "claude", &[]), &ctx)
                    .unwrap();
            let args = launch.args();
            assert_eq!(&args[args.len() - 4..], tail, "{adapter:?}");
            assert_eq!(&args[..2], ["-p", "--input-format"], "{adapter:?}");
            assert_eq!(args[args.len() - 5], "--no-chrome", "{adapter:?}");
            let spec = launch.preflight.as_ref().unwrap();
            assert_eq!(spec.version_args, vec!["--version".to_string()]);
            assert_eq!(spec.help_args, vec!["--help".to_string()]);
            let published = &launch.status().args;
            assert_eq!(&published[published.len() - 4..], tail, "{adapter:?}");
        }
    }
}
