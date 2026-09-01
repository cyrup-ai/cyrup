//! Seam tests drained from **`crates/cyrup-ext-subagents`** — all 35 files.
//!
//! What makes a test belong here: it drives a real subagent CHILD PROCESS, or it mutates this
//! process's environment so that the library's own in-process spawn resolver picks up a test
//! double. The fixture-bin files drive `cyrup-subagent-fixture` (scripted NDJSON on stdout,
//! deterministic signal handling) through `CYRUP_SUBAGENT_BINARY`;
//! `background_spawn_detached_integration` kills `cyrup-subagent-orchestrator-sim` mid-run to prove
//! the detached runner outlives its parent. None of that can be asserted in-process, which is
//! exactly why these 35 did not move into `src/` with the other 199.
//!
//! # What the migration changed, exhaustively
//!
//! Assertions were RELOCATED, never rewritten. Every `assert!`, every test body, and every
//! control flow is byte-identical to its pre-migration form. Only three things changed:
//!
//! 1. `PathBuf::from(env!("CARGO_BIN_EXE_cyrup-subagent-fixture"))` →
//!    `crate::support::bins::subagent_fixture()` (26 sites), and the one
//!    `CARGO_BIN_EXE_cyrup-subagent-orchestrator-sim` → `subagent_orchestrator_sim()`. Both
//!    accessors return `PathBuf`, the same type the `env!` expression produced, so no call site
//!    needed adjusting. The reason `env!` cannot stay is in `support/bins.rs`.
//!
//! 2. `#![cfg(feature = "test-fixtures")]` was REMOVED from the 23 files that carried it. In
//!    `cyrup-ext-subagents` that named the feature gating its own two `[[bin]]` targets, so the
//!    attribute meant "skip these tests when the fixture binary was not built". Re-spelled inside
//!    `cyrup-it` the identical text would name THIS crate's features, where no `test-fixtures`
//!    exists — so every one of those 23 modules would have compiled to NOTHING and passed
//!    vacuously. That is the invisible-skip failure the `it` gate exists to prevent. It is safe to
//!    drop outright because `cyrup-it`'s `build.rs` always builds both fixture binaries with
//!    `--features test-fixtures`; their availability is now a build-script postcondition rather
//!    than a compile-time cfg.
//!
//! 3. Two `#![cfg(unix)]` inner attributes became `#[cfg(unix)]` on the `mod` declarations below,
//!    which is the same gate expressed at the new nesting level.
//!
//! # Env mutation: one file left, and why
//!
//! This block used to warn that 33 of these files called `unsafe { std::env::set_var }` on THIS
//! process, each justifying it as "a test file is its own binary" — a justification the merge into
//! one binary silently voided, and which the per-file mutexes did not repair (33 distinct statics
//! guarding one shared environment is no mutual exclusion at all).
//!
//! That is no longer the shape. Every one of those files now names what it needs explicitly,
//! through seams the library already had or grew for the purpose:
//! `SubagentExtensionConfig::{spawn_command, roots, env_overrides}`,
//! `RunOptions::spawn_command`, `runner_main::run_with`, `spawn_detached_runner_with_command`'s
//! `env_overlay`, `NativeSupervisorChannel::with_root`, and the `_in`/`_with`/`_from` injected
//! cores across `nested_events`, `registration` and `paths`. No `unsafe` and no lock is involved.
//!
//! NOTHING in this binary mutates the process environment any more, so there is no `unsafe` and no
//! lock left to reason about. The last holdout was `background_cascade_integration`, whose root is
//! read MID-RUN by the cascade: `paths::Roots` is resolved once in `run_with` and carried on the
//! per-run `TurnLoopIo` that `check_stop_flag`, `check_timeout_flag`, `check_interrupt_flag` and
//! `settle_step_result` already share, so that read sees the caller's tree with no signature change
//! anywhere.
//!
//! So for THIS target, `cargo nextest run -p cyrup-it --features it` is the supported invocation
//! and the plain-`cargo test` fallback is NOT interchangeable with it. Repairing that properly
//! means moving the resolver onto injected config instead of ambient env — a source change in
//! `cyrup-ext-subagents`, well outside a test relocation.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[path = "../support/mod.rs"]
mod support;

// Grouped by BEHAVIOUR rather than by the source file each one touches — grouping by source file
// is how 310 files accumulated in the first place.

// ---- child process protocol: what the parent reads off a live child's stdio ----
mod child_protocol_stream_integration;
mod child_stderr_drain_integration;
mod child_written_output_authorship;
mod foreground_progress_stream_integration;

// ---- background runs: detachment, restart, signals, stop ----
mod background_cascade_integration;
mod background_runner_main_integration;
mod background_spawn_detached_integration;
mod run_state_signal_and_stop_parity;
mod startup_retry_lifecycle_integration;

// ---- exec: the synchronous run path, step chaining, parallelism, artifacts ----
mod artifacts_run_integration;
mod chain_step_child_detail_integration;
mod exec_run_sync_integration;
mod tool_parallel_chain_integration;

// ---- acceptance & verification ledger ----
mod acceptance_parser_state_model_interaction;
mod read_only_acceptance_inference;
// `unsafe { set_var }` + a real `sh` child; the original file's `#![cfg(unix)]` moved here.
#[cfg(unix)]
mod acceptance_memo_key_and_live_wiring;


// ---- installation: discovery, registration, the opt-in gate, end-to-end attach ----
mod discovery_project_root_wiring_integration;
mod extension_end_to_end_smoke;
mod management_actions_tool_dispatch_integration;
mod registration_commands_integration;
mod subagents_optin_gate_integration;
mod wait_tool_registration_integration;

// ---- companion subsystems: host services, intercom delivery, supervisor channel ----
mod companions_hostservices_proof;
mod companions_wiring_proof;
mod control_notice_pipeline_integration;
mod native_supervisor_channel_integration;
mod result_intercom_delivery_integration;

// ---- persona resolution, mode overrides, CYRUP_HOME sandboxing ----
mod cyrup_home_env_sandboxed_tests;
mod single_mode_overrides_integration;
mod subagent_persona_and_depth_integration;

// ---- operator surface: slash commands, prompt workflows, rendering, fleet inspection ----
mod fleet_inspector_integration;
mod prompt_workflow_commands_integration;
mod slash_command_dispatch_integration;
mod subagent_tool_renderer_integration;
