//! SUBA-074 — **how** a declared `runner:` is honoured, decided once, totally, and without I/O.
//!
//! ## The invariant, and the gate that used to fail open
//!
//! For every `Option<AgentRunnerConfig>` exactly one of three things holds: it is the native pi
//! child; it is an external profile **and this build has produced a complete launch plan for it**;
//! or it must be refused with a named reason. Stage 1 encoded that as
//! `AgentRunnerConfig::refusal_reason() -> Option<String>`, where `None` meant "spawn a native
//! child" — correct only because the supported set was EMPTY.
//!
//! The moment one adapter becomes supported, `None` starts meaning two different things ("this is
//! the native child" and "this is an external profile someone else will handle") and nothing in the
//! type system connects the second to an actual launch. A one-token edit widening the supported
//! guard from `ClaudeCode` to `ClaudeCode | ClaudeCodeWriter` while the dispatcher still matched
//! only `ClaudeCode` would silently reinstate this item's ORIGINAL bug: upstream forbids an external
//! profile from declaring `tools:`, so `AgentDefinition::tools` is `None`, which this crate reads as
//! "no allowlist restriction, all builtin tools available" (`discovery/types.rs:1025`) — a sandboxed
//! read-only foreign profile running as a full-capability native child in the workspace, with no
//! compiler complaint, no failing test outside that adapter's own, and no user-visible diagnostic.
//!
//! [`RunnerDispatch`] closes that by making possession of the launch plan the PROOF of support:
//! there is no `ExternalCli` variant without an [`ExternalCliLaunch`], the match below has no `_`
//! arm, and [`crate::exec::run_sync`] inspects the outcome in exactly one place. Adding a seventh
//! adapter id, or moving one out of the deferred set, is now a compile error in every place that
//! must change.
//!
//! ## Why the decision is pure
//!
//! Upstream's own contract says an external run resolves no model, no candidates and no thinking
//! ceiling (`api/preflight.ts:322-343` @v0.64.0), so this decision has to be taken BEFORE
//! `run_sync` builds its candidate list — beside the depth guard, not inside the ladder. Keeping it
//! a pure function over explicit inputs (no filesystem, no clock, no process environment) is what
//! makes every one of the eight `(runner.type, adapter)` outcomes — including each refusal's exact
//! wording — unit-testable without standing up a run.

use crate::exec::external_cli::{
    ExternalCliLaunch, ExternalCliLaunchContext, resolve_claude_code_launch, resolve_generic_launch,
};
use crate::runner::AgentRunnerConfig;
use crate::runner::contract::AdapterId;

/// What [`crate::exec::run_sync`] must do with a declared runner. Total by construction.
#[derive(Debug)]
pub enum RunnerDispatch {
    /// The native pi child — the spawn plan, the fallback ladder, everything unchanged.
    NativePi,
    /// An external CLI this build can execute. Holding the plan IS the proof of support.
    ExternalCli(Box<ExternalCliLaunch>),
    /// Not honourable here. The text is used verbatim by `pre_spawn_failure`, so the run fails with
    /// a named reason rather than being silently downgraded to a native child.
    Refused(String),
}

/// Decide how a declared runner runs. Pure: no filesystem, no clock, no `std::env`.
///
/// The `match` deliberately names every [`AdapterId`] and has no `_` arm.
#[must_use]
pub fn resolve_runner_dispatch(
    runner: Option<&AgentRunnerConfig>,
    ctx: &ExternalCliLaunchContext,
) -> RunnerDispatch {
    match runner {
        // "No runner declared" and `{type: pi}` are the same thing (`agents.ts:1855-1858`).
        None | Some(AgentRunnerConfig::Pi) => RunnerDispatch::NativePi,
        Some(AgentRunnerConfig::ExternalCli(cli)) => match cli.adapter {
            // The generic path — upstream's in-baseline runner, which needs no vendor CLI.
            None => RunnerDispatch::ExternalCli(Box::new(resolve_generic_launch(cli, ctx))),
            Some(adapter @ (AdapterId::ClaudeCode | AdapterId::ClaudeCodeWriter)) => {
                match resolve_claude_code_launch(adapter, cli, ctx) {
                    Ok(launch) => RunnerDispatch::ExternalCli(Box::new(launch)),
                    // Unreachable for the code-owned allowlist, but a launch that cannot be built
                    // is a refusal, never a fall-through to the native child.
                    Err(error) => RunnerDispatch::Refused(error),
                }
            }
            Some(
                adapter @ (AdapterId::CodexExec
                | AdapterId::CodexExecWriter
                | AdapterId::CursorAgent
                | AdapterId::CursorAgentWriter),
            ) => RunnerDispatch::Refused(unported_adapter_refusal(adapter)),
        },
        Some(AgentRunnerConfig::ExternalJob(job)) => {
            RunnerDispatch::Refused(unported_external_job_refusal(&job.provider))
        }
    }
}

/// The refusal for a code-owned adapter this build has not ported yet.
///
/// Each deferral has a reason, and neither is "it is big":
///
/// * **codex-exec / codex-exec-writer** add a second output channel — the `--output-last-message`
///   artifact read back under a size cap (`codex-exec-adapter.ts:56-74`) — and a second terminal
///   vocabulary (`turn.completed`/`turn.failed`/`error`).
/// * **cursor-agent / cursor-agent-writer** add prompt-file delivery, a 0700 handoff directory with
///   `--add-dir` when it falls outside the workspace, and the bounded-prefix oversized-line skip
///   (`cursor-agent-adapter.ts:34-113`) — the most intricate path in `runExternalCli`.
///
/// Both are landable on top of the runner that now exists; neither should be debugged in the same
/// change as the runner's own first end-to-end run.
#[must_use]
pub fn unported_adapter_refusal(adapter: AdapterId) -> String {
    format!(
        "Agent runner.type='external-cli' (adapter '{adapter}') is declared but the '{adapter}' \
         adapter is not yet supported by cyrup (SUBA-074). Refusing to launch rather than running \
         this profile as a full-capability native child."
    )
}

/// The refusal for `runner.type='external-job'`.
///
/// Deferred on a contract argument, not a size one: `api/external-job-provider.ts:1-2` @v0.64.0 is a
/// pure EMBEDDER REGISTRY (`EXTERNAL_JOB_PROVIDER_REGISTRY_KEY =
/// "pi-subagents.external-job-providers.v1"`) and **no provider ships in the upstream repo**.
/// Porting it into cyrup, which has no analogous host-registration surface, would produce a code
/// path that can never succeed — replacing today's honest "not supported" with a less honest "no
/// provider registered".
#[must_use]
pub fn unported_external_job_refusal(provider: &str) -> String {
    format!(
        "Agent runner.type='external-job' (provider '{provider}') is declared but not yet \
         supported by cyrup (SUBA-074). Refusing to launch rather than running this profile as a \
         full-capability native child."
    )
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
    use crate::runner::{ExternalCliRunner, ExternalJobRunner};

    fn cli(adapter: Option<AdapterId>) -> AgentRunnerConfig {
        AgentRunnerConfig::ExternalCli(ExternalCliRunner {
            adapter,
            command: "claude".to_string(),
            args: Vec::new(),
            prompt_delivery_stdin: false,
            capabilities: None,
        })
    }

    fn dispatch(runner: Option<&AgentRunnerConfig>) -> RunnerDispatch {
        resolve_runner_dispatch(runner, &ExternalCliLaunchContext::default())
    }

    /// The native arm is CHOSEN, not fallen into: an absent runner and an explicit `pi` runner both
    /// name it, and nothing else can reach it.
    #[test]
    fn only_an_absent_or_pi_runner_dispatches_to_the_native_child() {
        assert!(matches!(dispatch(None), RunnerDispatch::NativePi));
        assert!(matches!(
            dispatch(Some(&AgentRunnerConfig::Pi)),
            RunnerDispatch::NativePi
        ));
        for other in [
            cli(None),
            cli(Some(AdapterId::ClaudeCode)),
            cli(Some(AdapterId::CodexExec)),
            AgentRunnerConfig::ExternalJob(ExternalJobRunner {
                provider: "acme".to_string(),
                options: None,
            }),
        ] {
            assert!(
                !matches!(dispatch(Some(&other)), RunnerDispatch::NativePi),
                "{other:?} must never reach the native child"
            );
        }
    }

    /// The two SUPPORTED external paths yield a launch plan — which is the only way to be
    /// "supported" at all.
    #[test]
    fn the_generic_path_and_the_claude_code_adapter_resolve_to_a_launch() {
        let RunnerDispatch::ExternalCli(generic) = dispatch(Some(&cli(None))) else {
            panic!("the generic external-cli path must resolve to a launch");
        };
        assert_eq!(generic.status().adapter.id.wire(), "external-cli");

        for adapter in [AdapterId::ClaudeCode, AdapterId::ClaudeCodeWriter] {
            let RunnerDispatch::ExternalCli(launch) = dispatch(Some(&cli(Some(adapter)))) else {
                panic!("{adapter} must resolve to a launch");
            };
            assert_eq!(launch.status().adapter.id.wire(), adapter.wire());
            assert!(launch.status().safety.is_some());
        }
    }

    /// The DEFERRED adapters and the whole external-job protocol still refuse — by name, and
    /// without downgrading. Pinning the wording here is what keeps the deferral a decision rather
    /// than an assumption.
    #[test]
    fn the_deferred_adapters_and_external_job_still_refuse_by_name() {
        for adapter in [
            AdapterId::CodexExec,
            AdapterId::CodexExecWriter,
            AdapterId::CursorAgent,
            AdapterId::CursorAgentWriter,
        ] {
            let RunnerDispatch::Refused(reason) = dispatch(Some(&cli(Some(adapter)))) else {
                panic!("{adapter} must still refuse");
            };
            assert!(reason.contains("runner.type='external-cli'"), "{reason}");
            assert!(reason.contains(&format!("adapter '{adapter}'")), "{reason}");
            assert!(reason.contains("full-capability native child"), "{reason}");
        }

        let job = AgentRunnerConfig::ExternalJob(ExternalJobRunner {
            provider: "acme".to_string(),
            options: None,
        });
        let RunnerDispatch::Refused(reason) = dispatch(Some(&job)) else {
            panic!("external-job must still refuse");
        };
        assert!(reason.contains("runner.type='external-job'"), "{reason}");
        assert!(reason.contains("provider 'acme'"), "{reason}");
        assert!(reason.contains("full-capability native child"), "{reason}");
    }
}
