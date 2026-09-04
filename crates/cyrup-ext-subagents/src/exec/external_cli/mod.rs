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
#[derive(Debug, Clone, Default)]
pub struct ExternalCliLaunchContext {
    /// The child's working directory.
    pub cwd: PathBuf,
    /// Where the bounded stream logs and any adapter artifacts go.
    pub scratch_dir: PathBuf,
    /// The flat step index, which names those logs.
    pub step_index: usize,
    /// Upstream's `commandPrefixArgs` test seam (`claude-code-adapter.ts:84-85`): argv placed in
    /// FRONT of the adapter's own, so a test can point `command` at an interpreter and still
    /// exercise the real flags against a fake process. Empty in production.
    pub command_prefix_args: Vec<String>,
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
    let args = claude_code::launch_args(adapter, &ctx.command_prefix_args);
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

/// Execute an external-CLI runner and lower its outcome into a [`SingleResult`].
///
/// The result carries `model: None` and empty `attempted_models`/`model_attempts` — upstream
/// resolves NO model for an external runner at all (`api/preflight.ts:322-343`), and a "helpful"
/// ladder entry here would misreport which model produced the work.
pub async fn run_external_cli(
    agent: &AgentConfig,
    task: &str,
    opts: &RunOptions,
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

    let output_snapshot = crate::exec::output::snapshot_output_file(opts.output_path.as_deref());
    let scratch_dir = crate::background::attempt_scratch_dir(&opts.cwd);
    let step_index = opts.child_index.unwrap_or(0);

    // `buildExternalCliPrompt(step.systemPrompt ?? "", task)` (`subagent-runner.ts:1506`).
    let prompt_text = build_external_cli_prompt(&agent.system_prompt_body, task);
    let prepared = match delivery.prepare(&prompt_text) {
        Ok(prepared) => prepared,
        Err(error) => {
            return external_failure(agent, task, &status, None, error.to_string());
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
                return external_failure(agent, task, &status, None, error);
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
        saved_output_path.as_ref(),
        false,
        outcome.exit_code,
        agent.max_output,
        opts.output_mode,
    );

    SingleResult {
        agent: agent.name.clone(),
        task: task.to_string(),
        exit_code: outcome.exit_code,
        usage: Usage::default(),
        // Upstream resolves no model for an external runner at all.
        model: None,
        attempted_models: Vec::new(),
        model_attempts: Vec::new(),
        final_output,
        structured_output: None,
        acceptance: None,
        detached: false,
        interrupted: false,
        timed_out: outcome.timed_out,
        stopped: outcome.stopped,
        process_signal: outcome.process_signal.clone(),
        turn_budget: None,
        turn_budget_exceeded: false,
        wrap_up_requested: false,
        usage_budget: None,
        error,
        saved_output_path: saved_output_path.map(|path| path.display().to_string()),
        tool_calls: Vec::new(),
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
}
