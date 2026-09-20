//! Hop-2 detached-runner main loop (func-SA §5.4 R-SA-073..077/098..103; arch-SA §6.5).
//!
//! This is the single riskiest file in the crate: it is the integration point every other
//! background-subsystem module (`spawn_detached.rs`, `atomic.rs`, `control.rs`, `reconcile.rs`)
//! and the Phase 3 spawn boundary (`spawn/mod.rs`, `spawn/chain_graph.rs`, `spawn/parallel.rs`)
//! all feed into, and it is the ONE place the R-SA-077 "status.json before ResultFile, on EVERY
//! exit path" invariant must hold without exception. `crates/cyrup/src/subagent_runner_cmd.rs`
//! (a sibling crate, outside this one) is the sole caller: it selects the internal
//! `__subagent-runner --config <path>` subcommand and calls [`run`] directly — no separate
//! loader/interpreter hop, since `cyrup` is already one compiled binary.
//!
//! # The main-loop shape (arch-SA §6.5, restated exactly)
//!
//! ```text
//! read+delete config file
//!   -> resolve fork_context is already done (eager, by the orchestrator, R-SA-137) — this loop
//!      only reads the already-resolved session-file path per step, never re-derives it
//!   -> write initial RunStatus{state:Running,pid:self} via atomic.rs        (R-SA-075)
//!   -> spawn a control-inbox watcher task (uses control.rs)                 (R-SA-082)
//!   -> loop {
//!        check interrupted                                                 (R-SA-084)
//!        consume pending append requests via control.rs (re-scan disk)     (R-SA-095/096)
//!        if step cursor exhausted, break
//!        run the next step via the Phase-3 spawn boundary                  (R-SA-045..069)
//!        write status via atomic.rs
//!        advance cursor
//!      }
//!   -> compute terminal state
//!   -> write status.json THEN ResultFile, in that exact order,             (R-SA-077)
//!      on every single exit path (happy path, early return, error branch)
//!   -> exit
//! ```
//!
//! # R-SA-077's ordering invariant is enforced by construction, not by convention
//!
//! Every code path that can end this function's execution — the happy path (steps exhausted),
//! an interrupt (steps paused mid-flight), and an unrecoverable internal error (e.g. the runner
//! config fails to parse) — funnels through exactly one function, [`finish_run`](finish::finish_run), which performs
//! the `status.json`-write-THEN-`ResultFile`-write sequence unconditionally and returns `()`
//! (never a `Result` a caller could short-circuit past). [`run`] itself has no `return` statement
//! that bypasses `finish_run`: every `?`/early-return branch inside the loop body is caught by an
//! inner `Result`-returning helper ([`run_inner`](turn_loop::run_inner)) whose own `Err` is turned into a terminal
//! `Failed` status by [`run`]'s own tail, which then always calls `finish_run`. This mirrors this
//! crate's established "no silent bypass of a load-bearing ordering invariant" convention (compare
//! `exec/mod.rs`'s own R-SA-033 post-hoc-correction-must-run-after-completion-guard ordering,
//! enforced the same way: one funnel function, no early return around it).
//!
//! # Delete-then-act idempotency (R-SA-073's config file, mirroring control.rs's own R-SA-083)
//!
//! Reading the one-shot `runner-config.json` handoff file follows the identical delete-then-act
//! discipline `control.rs::consume_interrupt_request` already established for interrupt requests
//! (R-SA-083): the file's *content* is read first (needed to actually build the run), then the
//! file is deleted — and a SECOND call to [`read_and_delete_config`] against an already-consumed
//! config path (the file no longer exists) returns a typed "already consumed" outcome rather than
//! panicking or erroring loudly, so a hypothetical double-invocation of the runner subcommand
//! against the same config path (a supervisor retry, a test harness bug) degrades gracefully
//! instead of crashing. This is NOT the same ordering as `control.rs`'s interrupt consumption
//! (which reads-then-deletes so a lost race against a concurrent consumer still returns the
//! content) — here there is only ever one reader (the one runner process invoked with this exact
//! `--config` path), so a plain "read, then delete, tolerate NotFound on delete" sequence is
//! sufficient and matches R-SA-073's literal text ("the runner MUST delete this config file
//! immediately after reading it").
//!
//! # ResultsDir filesystem-watch completion notification (R-SA-098..103)
//!
//! This module owns only the *runner-side* half of R-SA-098's contract: [`run`] writes the
//! terminal [`super::ResultFile`] into `ResultsDir` as its very last file-writing act (R-SA-077),
//! which is what makes the orchestrator-side watch observable at all. The ORCHESTRATOR-side watch
//! primitive itself (installing a `notify` watcher over the whole `ResultsDir`, deduping by a
//! seen-set with a bounded TTL, R-SA-099, classifying terminal outcomes, R-SA-100, and bounding
//! retry-in-place on processing failure, R-SA-102) runs in the **orchestrator** process — never
//! the detached runner process this file's main loop (`run`) itself executes in — and lives in
//! the sibling module [`crate::background::watch`], per arch-SA §2.2's module layout. See that
//! module's own docs for the full R-SA-098..103 contract.

mod config;
mod control_watcher;
mod entry;
mod events;
mod executor;
mod finish;
mod settle;
mod status;
mod turn_loop;

pub use config::{ConfigConsumeOutcome, RunnerConfig, read_and_delete_config};
pub use entry::{RunnerOverrides, run, run_with};
pub use events::{ASYNC_EVENTS_MAX_BYTES_ENV, resolve_async_events_cap_bytes};
// `mod finish` is private, so a `pub(crate)` item inside it is unreachable from outside this
// module tree. SCOPE_8's reconciler (`extension/executor/workflow_detach/`) is the first
// production caller of the settlement-plan stamper, which is why the pair is re-exported here
// rather than the caller reaching for `finish_run` — see that module's own write-path doc for why
// `finish_run` itself must NOT be widened or called.
pub(crate) use executor::ExecSingleStepExecutor;
pub(crate) use finish::{WorkflowResultFields, apply_workflow_settlement_plan};

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use crate::background::atomic::write_atomic_json;
    use crate::background::{ResultFile, RunId, RunMode, RunPaths, RunState, RunStatus};
    use crate::exec::SingleResult;
    use crate::spawn::chain_graph::{RunnerStep, SingleStepSpec};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    /// A minimal resolved persona, for the tests that need a dispatch to get PAST the
    /// persona-map lookup and reach the spawn.
    pub(super) fn resolved_persona(name: &str) -> crate::exec::ResolvedAgentPersona {
        crate::exec::ResolvedAgentPersona {
            file_path: None,
            acceptance_role: None,
            default_acceptance: None,
            name: name.to_string(),
            model: Some(cyrup_core::ModelId::from("fixture-model")),
            model_provider: None,
            fallback_models: Vec::new(),
            thinking: None,
            system_prompt_mode: crate::discovery::types::SystemPromptMode::Replace,
            system_prompt_body: String::new(),
            tools: None,
            extensions: None,
            subagent_only_extensions: Vec::new(),
            exclude_tools: Vec::new(),
            allow_nested_subagents: None,
            output: None,
            inherit_project_context: false,
            inherit_skills: true,
            skills: Vec::new(),
            completion_guard: Some(false),
            max_subagent_depth: None,
            default_context: None,
            memory: None,
            tool_budget: None,
            runner: None,
        }
    }

    pub(super) fn single_step(agent: &str, task: &str) -> SingleStepSpec {
        SingleStepSpec {
            skills: None,
            session_dir: None,
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

    // ---------------------------------------------------------------------------------------
    // run(): full run through the scripted fixture — status-then-result ordering (happy path,
    // forced-error path, missing-config path) and R-SA-096 disk-re-scan append consumption.
    //
    // These live in `tests/background_runner_main_integration.rs`, NOT here, for the identical
    // reason `spawn_detached.rs`'s own module docs give for `spawn_detached_runner`'s fixture-
    // backed proof: `CARGO_BIN_EXE_cyrup-subagent-fixture` is only defined for ordinary Cargo
    // integration tests (files under `tests/`), never for a library's own `#[cfg(test)]` unit
    // tests compiled into `src/`, so `env!("CARGO_BIN_EXE_cyrup-subagent-fixture")` cannot resolve
    // in this module at all. Separately (and independently sufficient on its own), those tests
    // must mutate `CYRUP_SUBAGENT_BINARY`/`CYRUP_SUBAGENT_FIXTURE_SCRIPT` via `unsafe { std::env::
    // set_var/remove_var }` (Rust 2024 requires `unsafe` for either), which this crate's own
    // `#![forbid(unsafe_code)]` (`src/lib.rs`) blocks even inside a `#[cfg(test)]` module — a
    // `tests/*.rs` file is its own separate compilation unit, not subject to the library crate's
    // `forbid` attribute, exactly like `tests/background_spawn_detached_integration.rs`'s own
    // established precedent for the identical constraint.
    // ---------------------------------------------------------------------------------------

    // ---------------------------------------------------------------------------------------
    // finish_run: double-invocation idempotency — a second terminal write against a run id that
    // ALREADY has an authoritative ResultFile on disk must be a no-op, never an overwrite. This is
    // the `finish_run`-level half of this module's double-invocation contract (the
    // `read_and_delete_config`-level half is proven above); together they cover the full `run()`
    // double-invocation scenario without needing the fixture binary, since `finish_run` is called
    // directly here rather than driving the whole `run_inner` step loop.
    // ---------------------------------------------------------------------------------------

    pub(super) fn run_paths_in(dir: &std::path::Path, run_id: &RunId) -> RunPaths {
        let async_root = dir.join("async");
        let results_dir = dir.join("results");
        RunPaths::for_run(&async_root, &results_dir, run_id)
    }

    // ---------------------------------------------------------------------------------------
    // R-SA-097 root attachment: an ImportAsyncRoot step becomes a chain's first step by POLLING
    // another already-completed run — no subprocess spawned, so provable in-module without the
    // fixture binary (mirrors pi chain-root-attachment.ts / subagent-runner.ts:1153).
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn an_attached_async_root_becomes_a_chains_first_step() {
        let dir = tempfile::tempdir().expect("real tempdir");

        // The TARGET (already-launched) run: write its terminal status + ResultFile into its own
        // async-root/results-dir, distinct from THIS chain's own artifact roots.
        let target_async = dir.path().join("target-async");
        let target_results = dir.path().join("target-results");
        let target_id = RunId::from_token("target-root");
        let target_paths = RunPaths::for_run(&target_async, &target_results, &target_id);
        tokio::fs::create_dir_all(&target_paths.run_dir)
            .await
            .expect("mkdir target run_dir");
        tokio::fs::create_dir_all(&target_results)
            .await
            .expect("mkdir target results_dir");

        let mut target_status = RunStatus::queued(target_id.clone(), RunMode::Single, Some(4321));
        target_status
            .advance_state(RunState::Running)
            .expect("Queued -> Running");
        target_status
            .advance_state(RunState::Complete)
            .expect("Running -> Complete");
        write_atomic_json(&target_paths.status, &target_status)
            .await
            .expect("write target status");
        let target_result = ResultFile {
            schedule_origin: None,
            id: target_id.clone(),
            run_id: target_id.clone(),
            agent: "researcher".to_string(),
            mode: RunMode::Single,
            state: RunState::Complete,
            success: true,
            cwd: dir.path().to_path_buf(),
            session_file: None,
            session_id: crate::identity::SessionId::parse("test-session"),
            completion_owner_id: None,
            results: vec![SingleResult {
                // SUBA-021: no usage budget on this path (see the field doc).
                usage_budget: None,
                turn_budget: None,
                turn_budget_exceeded: false,
                wrap_up_requested: false,
                child_run_id: None,
                agent: "researcher".to_string(),
                task: "research the topic".to_string(),
                exit_code: 0,
                usage: cyrup_core::Usage::default(),
                turns: 0,
                model: None,
                attempted_models: Vec::new(),
                model_attempts: Vec::new(),
                final_output: Some("root output".to_string()),
                structured_output: None,
                session_file: None,
                output_state: Default::default(),
                structured_output_path: None,
                artifact_paths: None,
                transcript_path: None,
                transcript_error: None,
                acceptance: None,
                detached: false,
                interrupted: false,
                timed_out: false,
                timeout_recovery: None,
                context_overflow: false,
                stopped: false,
                process_signal: None,
                error: None,
                saved_output_path: None,
                tool_calls: Vec::new(),
                output_truncated: false,
                control_events: Vec::new(),
                progress: None,
                runner: None,
                external_process: None,
                // Test fixture: no child was planned, so there is no surface to report.
                tool_surface: crate::exec::tool_surface::ResolvedToolSurface::default(),
            }],
            workflow_children: None,
            workflow_receipt: None,
        };
        write_atomic_json(&target_paths.legacy_result_root, &target_result)
            .await
            .expect("write target result");

        // THIS chain: a single ImportAsyncRoot step attaching the target as its first step.
        let run_id = RunId::from_token("attaching-chain");
        let run_paths = run_paths_in(dir.path(), &run_id);
        tokio::fs::create_dir_all(&run_paths.run_dir)
            .await
            .expect("mkdir run_dir");
        tokio::fs::create_dir_all(dir.path().join("results"))
            .await
            .expect("mkdir results_dir");

        let config = RunnerConfig {
            runner_process_instance_id: None,
            revival_lease: None,
            // SUBA-021: unbudgeted on this path (see the field doc).
            usage_budget: None,
            turn_budget: None,
            permission_rules: None,
            // SUBA-N03: this fixture exercises neither the run-level timeout nor `share`/artifacts, so it
            // carries the same values an older on-disk config deserializes to (`#[serde(default)]`).
            timeout_ms: None,
            deadline_at_ms: None,
            share: None,
            artifacts_dir: None,
            artifact_config: crate::artifacts::ArtifactConfig::default(),
            run_id: run_id.clone(),
            mode: RunMode::Chain,
            steps: vec![RunnerStep::ImportAsyncRoot(
                crate::spawn::chain_graph::ImportAsyncRootSpec {
                    run_id: "target-root".to_string(),
                    async_root: target_async.clone(),
                    results_dir: target_results.clone(),
                    index: 0,
                    agent: "attached-root".to_string(),
                    output: Some("rootOut".to_string()),
                },
            )],
            cwd: dir.path().to_path_buf(),
            session_file: None,
            session_id: Some("test-session".to_string()),
            completion_owner_id: None,
            global_concurrency_limit: 20,
            worktree_base_dir: None,
            max_subagent_depth: 2,
            async_root: PathBuf::new(),
            results_dir: PathBuf::new(),
            resolved_agents: BTreeMap::new(),
            original_task: String::new(),
            chain_dir: None,
            orchestrator_intercom_target: None,
            inherited_session_model: None,
            inherited_session_thinking: None,
            host_available_builtins: None,
            model_scope: None,
            nested_route: None,
            nested_self: None,
            dynamic_fanout_max_items: None,
            control: None,
            include_progress: None,
        };
        let cfg_path = run_paths.run_dir.join("runner-config.json");
        write_atomic_json(&cfg_path, &config)
            .await
            .expect("write config");

        let outcome = run(&cfg_path, &run_paths).await;
        assert!(
            outcome.is_ok(),
            "run() never returns Err to its caller: {outcome:?}"
        );

        let result_file: ResultFile = serde_json::from_slice(
            &tokio::fs::read(&crate::background::result_index::owned_payload_path(
                &run_paths.results_dir,
                &crate::identity::SessionId::parse("test-session").expect("non-empty"),
                &run_id,
            ))
            .await
            .expect("terminal ResultFile must exist"),
        )
        .expect("valid JSON");

        assert_eq!(
            result_file.state,
            RunState::Complete,
            "attached root imported as success"
        );
        assert!(result_file.success);
        assert_eq!(
            result_file.results.len(),
            1,
            "the attached root IS the chain's first step"
        );
        let first = &result_file.results[0];
        assert_eq!(
            first.agent, "researcher",
            "the imported step takes the TARGET child's own agent, not the step's display name"
        );
        assert_eq!(first.final_output.as_deref(), Some("root output"));
        assert_eq!(first.exit_code, 0);
    }
}
