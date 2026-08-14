//! Seam tests drained from **`crates/cyrup-permission-system`** — 12 files.
//!
//! What makes a test belong here: it drives the forwarding path across a real process boundary —
//! a spawned child that asks, a parent that answers, and the persistence/dedup behaviour observed
//! between them.
//!
//! Migration notes:
//!
//! * This crate had TWO `spawn_child`/`wait_child` helpers with **divergent timeout constants**
//!   (8_000 vs 20_000 ms) — which is precisely how open item PERM-022 came to exist. They collapse
//!   into one helper here. Pick the constant deliberately and say why in a comment; do not average
//!   them, and do not silently keep both.
//!   **Done** — see [`forwarding_common`], which states the choice and its reason. The child-side
//!   wait bounds (8_000 / 20_000 / 1_200 ms) are call-site arguments and stayed at their call sites;
//!   only the parent-side reaper's poll cadence (40 ms vs 25 ms) was ever duplicated.
//! * `prompt_dedup.rs` does NOT belong in this target: moving it into `src/` is itself the
//!   prescribed fix for open item PERM-020 (the integration binary "cannot reach the crate-private
//!   `ext_config::env_lock()`", `10-cyrup-permission-system.md:357`). If it shows up in a
//!   migration batch aimed here, push back.
//!   **Pushed back** — it stays at `crates/cyrup-permission-system/tests/prompt_dedup.rs`, along
//!   with `forwarding_persist.rs`, which has the identical defect. Both call
//!   `std::env::set_var("CYRUP_SUBAGENT_CHILD", "1")` on the PROCESS, each justified in its own
//!   source by a comment reading "this is the one test binary in this file, so mutating process env
//!   here is race-free". That premise is true only while each is its own binary and is exactly what
//!   this consolidation destroys (§4 R2). It is not a theoretical race: an ambient
//!   `CYRUP_SUBAGENT_CHILD=1` makes `prompt_decision`'s `has_ui || is_subagent_child() || yolo`
//!   pre-check pass, so the headless fail-CLOSE that [`audit_logging`],
//!   [`permanent_approvals_file_is_inert`] and [`gate_integration`] each assert on stops happening
//!   and those tests go red on a correct build. Neither file crosses a process boundary, so neither
//!   is a seam test to begin with; both belong in `src/` under `#[cfg(test)]`, which is also where
//!   PERM-020's fix lands.
//! * `CYRUP_PERMISSION_SYSTEM` is an opt-in gate that is exported on the maintainer's machine.
//!   Every child built through `support::scratch::Scratch::command` already has it cleared; a test
//!   that needs it ON must set it on that child's `Command`, never on this process.
//! * The two re-exec'd child roles are named by their FULL libtest path now
//!   (`forwarding_subprocess::forwarding_child_role_entry`,
//!   `forwarding_spawn_env::spawn_env_child_role_entry`) because they are modules of one binary
//!   rather than binaries. A bare name makes `--exact` match nothing, and a child that runs zero
//!   tests exits 0 — which every one of those tests would read as "the tool was allowed".

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[path = "../support/mod.rs"]
mod support;

mod forwarding_common;

mod audit_logging;
mod config_command;
mod context_hygiene;
mod empty_command_truthiness;
mod forwarding_spawn_env;
mod forwarding_subprocess;
mod gate_integration;
mod human_dialog;
mod layers_wired;
mod permanent_approvals_file_is_inert;
